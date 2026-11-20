/*
 * SPDX-License-Identifier: Apache-2.0
 * Copyright (c) 2026 Kari Vierimaa, Kempele
 *
 * HotWire Oscilloscope — 3-Channel 12-bit GDMA ADC Oscilloscope Firmware
 * Target: Seeed Studio XIAO ESP32-C6 (RISC-V)
 *
 * Architecture:
 * - GDMA Continuous ADC (`esp_adc/adc_continuous.h`) up to 83.3 kSPS (ADC1_CH0..2 on D0..D2)
 * - O(1) eFuse Curve-Fitting 4096-entry mV LUT for calibrated voltage output
 * - Pre-trigger circular ring buffer + Rising/Falling Edge Trigger Engine
 * - Test Signal Generator: LEDC hardware PWM on D6 (GPIO16) at 1 / 5 / 10 / 50 kHz
 *
 * Arduino IDE Settings (arduino-esp32 3.x / ESP-IDF 5.x):
 *   - Board: "XIAO_ESP32C6"
 *   - USB CDC On Boot: "Enabled"
 */

#include <Arduino.h>
#include <string.h>
#include "esp_adc/adc_continuous.h"
#include "esp_adc/adc_cali.h"
#include "esp_adc/adc_cali_scheme.h"
#include "esp_system.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/semphr.h"

#ifndef CONFIG_IDF_TARGET_ESP32C6
  #error "This sketch targets Seeed Studio XIAO ESP32-C6."
#endif

#define HOTWIRE_FW_VERSION "1.0.0-scope-c6"
#define HOTWIRE_BOARD_NAME "XIAO-ESP32-C6"

// XIAO ESP32-C6 Analog Pins: D0=GPIO0(CH0), D1=GPIO1(CH1), D2=GPIO2(CH2)
static const adc_channel_t SCOPE_ADC_CHANNELS[3] = {
  ADC_CHANNEL_0, ADC_CHANNEL_1, ADC_CHANNEL_2
};
static const int TEST_GEN_PIN = 16; // D6 on XIAO ESP32-C6

static const uint8_t SYNC_0 = 0xAA;
static const uint8_t SYNC_1 = 0x55;

static const uint8_t CMD_PING_STATUS = 0x01;
static const uint8_t CMD_GET_CAPS    = 0x02;
static const uint8_t CMD_SCOPE_CFG   = 0x40;
static const uint8_t CMD_SCOPE_TRIG  = 0x41;
static const uint8_t CMD_SCOPE_RUN   = 0x42;
static const uint8_t CMD_BOOTLOADER  = 0xF0;

static const uint8_t TYPE_BOARD_CAPS = 0x83;
static const uint8_t TYPE_SCOPE_DATA = 0x86;

static const uint8_t FLAG_TRIGGERED     = 0x01;
static const uint8_t FLAG_CALIBRATED_MV = 0x04;
static const uint8_t FLAG_ADC_SATURATED = 0x08;
static const uint8_t FLAG_PACKED_12BIT  = 0x10;

// ESP32-C6 digital SAR controller max spec is 83,333 Hz
static const uint32_t RATE_TABLE[7]   = { 10000, 20000, 50000, 83333, 83333, 83333, 83333 };
static const uint16_t DEPTH_TABLE[4]  = { 512, 1024, 2048, 4096 };
static const uint16_t VREF_TABLE[4]   = { 950, 1250, 1750, 3100 };
static const adc_atten_t ATTEN_TABLE[4] = {
  ADC_ATTEN_DB_0, ADC_ATTEN_DB_2_5, ADC_ATTEN_DB_6, ADC_ATTEN_DB_12
};

struct ScopeConfig {
  uint8_t  rate_code;
  uint8_t  channel_mask;
  uint8_t  atten_code;
  uint8_t  depth_code;
  uint8_t  trig_mode;
  uint8_t  trig_edge;
  uint8_t  trig_ch;
  uint16_t trig_level_mv;
  uint8_t  pre_trig_pct;
  uint8_t  run_mode;
  uint8_t  target_fps;
  uint8_t  test_gen_code;
  bool     cali_enabled;
  bool     adc_dirty;
};

static volatile ScopeConfig g_cfg = {
  3, 0x03, 3, 1,
  0, 0, 0, 1550, 50,
  0, 15, 1, true, true
};

static SemaphoreHandle_t g_serial_mutex = nullptr;
static adc_continuous_handle_t g_adc_handle = nullptr;
static bool g_adc_running = false;

static uint16_t g_cali_lut[4096];
static uint8_t  g_cali_lut_atten = 0xFF;

static constexpr size_t DMA_READ_BYTES = 2048;
static uint8_t  g_dma_buf[DMA_READ_BYTES];
static constexpr size_t RING_CAP = 8192;
static uint16_t g_ring[3][RING_CAP];
static uint8_t  g_frame_payload[16 + (3 * 4096 * 3) / 2];

static uint8_t compute_xor(const uint8_t* data, size_t len) {
  uint32_t acc = 0;
  size_t i = 0;
  while (i + 4 <= len) {
    uint32_t w;
    memcpy(&w, data + i, 4);
    acc ^= w;
    i += 4;
  }
  uint8_t x = (uint8_t)((acc) ^ (acc >> 8) ^ (acc >> 16) ^ (acc >> 24));
  while (i < len) x ^= data[i++];
  return x;
}

static void serial_write_all(const uint8_t* data, size_t len) {
  size_t sent = 0;
  uint32_t start_ms = millis();
  while (sent < len) {
    size_t chunk = min<size_t>(1024, len - sent);
    size_t written = Serial.write(data + sent, chunk);
    if (written > 0) {
      sent += written;
      start_ms = millis();
    } else {
      if ((millis() - start_ms) > 250) break;
      vTaskDelay(pdMS_TO_TICKS(1));
    }
  }
}

static void send_frame(uint8_t type_id, const uint8_t* payload, uint16_t len) {
  uint8_t hdr[5] = {
    SYNC_0, SYNC_1, type_id,
    (uint8_t)(len & 0xFF), (uint8_t)((len >> 8) & 0xFF)
  };
  uint8_t chk = compute_xor(hdr, 5) ^ compute_xor(payload, len);

  if (g_serial_mutex) xSemaphoreTake(g_serial_mutex, portMAX_DELAY);
  serial_write_all(hdr, 5);
  if (len > 0) serial_write_all(payload, len);
  serial_write_all(&chk, 1);
  if (g_serial_mutex) xSemaphoreGive(g_serial_mutex);
}

static void send_capabilities() {
  const char* json =
    "{\"board\":\"" HOTWIRE_BOARD_NAME "\","
    "\"firmware\":\"" HOTWIRE_FW_VERSION "\","
    "\"scope\":true,\"max_channels\":3,\"max_rate_hz\":83333,\"adc_bits\":12}";
  send_frame(TYPE_BOARD_CAPS, (const uint8_t*)json, (uint16_t)strlen(json));
}

static void build_cali_lut(uint8_t atten_code) {
  if (atten_code > 3) atten_code = 3;
  if (g_cali_lut_atten == atten_code) return;

  adc_cali_handle_t cali = nullptr;
  adc_cali_curve_fitting_config_t cali_cfg = {};
  cali_cfg.unit_id = ADC_UNIT_1;
  cali_cfg.atten = ATTEN_TABLE[atten_code];
  cali_cfg.bitwidth = ADC_BITWIDTH_12;

  bool has_cali = (adc_cali_create_scheme_curve_fitting(&cali_cfg, &cali) == ESP_OK);
  uint16_t vref = VREF_TABLE[atten_code];

  for (int raw = 0; raw < 4096; ++raw) {
    int mv = 0;
    if (has_cali && adc_cali_raw_to_voltage(cali, raw, &mv) == ESP_OK) {
      g_cali_lut[raw] = (uint16_t)constrain(mv, 0, 3300);
    } else {
      g_cali_lut[raw] = (uint16_t)((raw * (uint32_t)vref) / 4095U);
    }
  }
  if (has_cali && cali) adc_cali_delete_scheme_curve_fitting(cali);
  g_cali_lut_atten = atten_code;
}

static void apply_test_generator(uint8_t code) {
  static const uint32_t freqs[5] = { 0, 1000, 5000, 10000, 50000 };
  static uint8_t s_last_code = 0xFF;
  if (code > 4) code = 0;
  if (code == s_last_code) return;
  if (s_last_code != 0xFF && s_last_code != 0) {
    ledcDetach(TEST_GEN_PIN);
  }
  s_last_code = code;
  if (code == 0) {
    pinMode(TEST_GEN_PIN, INPUT);
  } else {
    ledcAttach(TEST_GEN_PIN, freqs[code], 8);
    ledcWrite(TEST_GEN_PIN, 128);
  }
}

static void stop_adc() {
  if (g_adc_handle) {
    if (g_adc_running) {
      adc_continuous_stop(g_adc_handle);
      g_adc_running = false;
    }
    adc_continuous_deinit(g_adc_handle);
    g_adc_handle = nullptr;
  }
}

static void configure_adc(uint8_t rate_code, uint8_t ch_mask, uint8_t atten_code) {
  stop_adc();
  build_cali_lut(atten_code);

  ch_mask &= 0x07;
  if (ch_mask == 0) ch_mask = 0x01;
  uint32_t rate = RATE_TABLE[min<uint8_t>(rate_code, 6)];

  adc_continuous_handle_cfg_t handle_cfg = {};
  handle_cfg.max_store_buf_size = 8192;
  handle_cfg.conv_frame_size = DMA_READ_BYTES;
  if (adc_continuous_new_handle(&handle_cfg, &g_adc_handle) != ESP_OK) {
    g_adc_handle = nullptr;
    return;
  }

  adc_digi_pattern_config_t patterns[3] = {};
  uint8_t pat_cnt = 0;
  for (uint8_t ch = 0; ch < 3; ++ch) {
    if (ch_mask & (1 << ch)) {
      patterns[pat_cnt].atten = ATTEN_TABLE[min<uint8_t>(atten_code, 3)];
      patterns[pat_cnt].channel = SCOPE_ADC_CHANNELS[ch];
      patterns[pat_cnt].unit = ADC_UNIT_1;
      patterns[pat_cnt].bit_width = ADC_BITWIDTH_12;
      pat_cnt++;
    }
  }

  adc_continuous_config_t dig_cfg = {};
  dig_cfg.pattern_num = pat_cnt;
  dig_cfg.adc_pattern = patterns;
  dig_cfg.sample_freq_hz = rate;
  dig_cfg.conv_mode = ADC_CONV_SINGLE_UNIT_1;
  dig_cfg.format = ADC_DIGI_OUTPUT_FORMAT_TYPE2;

  if (adc_continuous_config(g_adc_handle, &dig_cfg) != ESP_OK) {
    stop_adc();
    return;
  }
  if (adc_continuous_start(g_adc_handle) == ESP_OK) {
    g_adc_running = true;
  }
}

static void scope_capture_task(void* arg) {
  (void)arg;
  uint32_t last_tx_ms = 0;

  while (true) {
    uint8_t run_mode = g_cfg.run_mode;
    if (run_mode == 0) {
      if (g_adc_running) stop_adc();
      vTaskDelay(pdMS_TO_TICKS(10));
      continue;
    }

    if (g_cfg.adc_dirty || !g_adc_running) {
      g_cfg.adc_dirty = false;
      configure_adc(g_cfg.rate_code, g_cfg.channel_mask, g_cfg.atten_code);
    }

    if (!g_adc_running || !g_adc_handle) {
      vTaskDelay(pdMS_TO_TICKS(20));
      continue;
    }

    uint8_t  ch_mask = g_cfg.channel_mask & 0x07;
    if (ch_mask == 0) ch_mask = 0x01;
    uint8_t  active_cnt = __builtin_popcount(ch_mask);
    uint8_t  depth_c = g_cfg.depth_code;
    uint16_t want_samples = DEPTH_TABLE[min<uint8_t>(depth_c, 3)];
    uint8_t  trig_mode = g_cfg.trig_mode;
    uint8_t  trig_edge = g_cfg.trig_edge;
    uint8_t  trig_ch_raw = g_cfg.trig_ch;
    uint8_t  trig_ch = min<uint8_t>(trig_ch_raw, 2);
    if ((ch_mask & (1 << trig_ch)) == 0) {
      for (uint8_t c = 0; c < 3; ++c) {
        if (ch_mask & (1 << c)) { trig_ch = c; break; }
      }
    }
    uint16_t trig_mv = g_cfg.trig_level_mv;
    uint8_t  pre_pct = g_cfg.pre_trig_pct;
    uint16_t pre_samples = (uint16_t)(((uint32_t)want_samples * min<uint8_t>(pre_pct, 75)) / 100U);
    uint16_t post_samples = want_samples - pre_samples;
    bool     cali_en = g_cfg.cali_enabled;

    uint8_t  fps_c = g_cfg.target_fps;
    uint32_t min_interval_ms = 1000U / max<uint8_t>(fps_c, 1);
    if (run_mode == 1 && (millis() - last_tx_ms) < min_interval_ms) {
      uint32_t dummy_ret = 0;
      adc_continuous_read(g_adc_handle, g_dma_buf, DMA_READ_BYTES, &dummy_ret, 5);
      continue;
    }

    size_t write_idx[3] = { 0, 0, 0 };
    size_t ch_trig_pos[3] = { 0, 0, 0 };
    size_t total_trig_ch = 0;
    bool triggered = false;
    size_t post_collected = 0;
    bool saturated = false;

    uint8_t  rate_c_init = g_cfg.rate_code;
    uint32_t per_ch_est = max<uint32_t>(RATE_TABLE[min<uint8_t>(rate_c_init, 6)] / active_cnt, 1000);
    uint32_t window_ms = (uint32_t)(((uint64_t)want_samples * 1000ULL) / per_ch_est);
    uint32_t acq_start_ms = millis();
    uint32_t timeout_ms = (trig_mode == 1 || run_mode == 2) ? max<uint32_t>(1200, window_ms + 400) : max<uint32_t>(140, window_ms + 120);
    static const int16_t TRIG_HYST_MV = 15;
    bool trig_armed = false;

    while (g_cfg.run_mode != 0 && !g_cfg.adc_dirty) {
      uint32_t ret_num = 0;
      esp_err_t err = adc_continuous_read(g_adc_handle, g_dma_buf, DMA_READ_BYTES, &ret_num, 20);
      if (err == ESP_OK && ret_num > 0) {
        for (uint32_t i = 0; i + SOC_ADC_DIGI_RESULT_BYTES <= ret_num; i += SOC_ADC_DIGI_RESULT_BYTES) {
          const adc_digi_output_data_t* p = (const adc_digi_output_data_t*)&g_dma_buf[i];
          uint8_t ch = p->type2.channel;
          if (ch >= 3 || (ch_mask & (1 << ch)) == 0) continue;

          uint16_t raw = p->type2.data & 0x0FFF;
          if (raw <= 2 || raw >= 4093) saturated = true;
          uint16_t mv = g_cali_lut[raw];
          uint16_t val = cali_en ? mv : raw;

          size_t pos = write_idx[ch] & (RING_CAP - 1);
          g_ring[ch][pos] = val;
          write_idx[ch]++;

          if (ch == trig_ch) {
            total_trig_ch++;
            if (!triggered) {
              if (trig_mode == 3) {
                if (total_trig_ch >= pre_samples) {
                  triggered = true;
                  for (uint8_t c = 0; c < 3; ++c) {
                    ch_trig_pos[c] = (write_idx[c] > 0 ? (write_idx[c] - 1) : 0) & (RING_CAP - 1);
                  }
                }
              } else {
                int16_t mv_i = (int16_t)mv;
                int16_t th_i = (int16_t)trig_mv;
                if (!trig_armed) {
                  if ((trig_edge == 0 && mv_i <= th_i - TRIG_HYST_MV) ||
                      (trig_edge == 1 && mv_i >= th_i + TRIG_HYST_MV)) {
                    trig_armed = true;
                  }
                } else if (total_trig_ch >= pre_samples) {
                  bool crossed = (trig_edge == 0) ? (mv_i >= th_i) : (mv_i <= th_i);
                  if (crossed) {
                    triggered = true;
                    for (uint8_t c = 0; c < 3; ++c) {
                      ch_trig_pos[c] = (write_idx[c] > 0 ? (write_idx[c] - 1) : 0) & (RING_CAP - 1);
                    }
                  }
                }
              }
            } else {
              post_collected++;
              if (post_collected >= post_samples + 2) break;
            }
          }
        }
      }

      if (triggered && post_collected >= post_samples + 2) break;
      if (!triggered && (millis() - acq_start_ms) >= timeout_ms) {
        if (trig_mode == 0 && total_trig_ch >= want_samples) {
          for (uint8_t c = 0; c < 3; ++c) {
            size_t w = write_idx[c];
            ch_trig_pos[c] = (w >= post_samples ? (w - post_samples) : 0) & (RING_CAP - 1);
          }
          break;
        } else if (trig_mode == 1) {
          acq_start_ms = millis();
          vTaskDelay(pdMS_TO_TICKS(5));
        } else {
          break;
        }
      }
    }

    if (g_cfg.run_mode == 0 || g_cfg.adc_dirty) continue;

    uint8_t  rate_c = g_cfg.rate_code;
    uint32_t total_rate = RATE_TABLE[min<uint8_t>(rate_c, 6)];
    uint32_t per_ch_rate = max<uint32_t>(total_rate / active_cnt, 1000);
    uint8_t  atten_c = g_cfg.atten_code;
    uint8_t  atten_code = min<uint8_t>(atten_c, 3);
    uint16_t vref_mv = VREF_TABLE[atten_code];

    uint8_t flags = FLAG_PACKED_12BIT;
    if (triggered && trig_mode != 3) flags |= FLAG_TRIGGERED;
    if (cali_en) flags |= FLAG_CALIBRATED_MV;
    if (saturated) flags |= FLAG_ADC_SATURATED;

    uint8_t* dst = &g_frame_payload[16];
    size_t dst_len = 0;
    bool has_pending = false;
    uint16_t s0 = 0;
    for (uint8_t ch = 0; ch < 3; ++ch) {
      if ((ch_mask & (1 << ch)) == 0) continue;
      size_t start_pos = (ch_trig_pos[ch] + RING_CAP - pre_samples) & (RING_CAP - 1);
      for (uint16_t k = 0; k < want_samples; ++k) {
        uint16_t s = g_ring[ch][(start_pos + k) & (RING_CAP - 1)] & 0x0FFF;
        if (!has_pending) {
          s0 = s;
          has_pending = true;
        } else {
          dst[dst_len++] = (uint8_t)(s0 & 0xFF);
          dst[dst_len++] = (uint8_t)(((s0 >> 8) & 0x0F) | ((s & 0x0F) << 4));
          dst[dst_len++] = (uint8_t)((s >> 4) & 0xFF);
          has_pending = false;
        }
      }
    }
    if (has_pending) {
      dst[dst_len++] = (uint8_t)(s0 & 0xFF);
      dst[dst_len++] = (uint8_t)((s0 >> 8) & 0x0F);
    }

    uint16_t payload_len = 16 + (uint16_t)dst_len;
    memcpy(&g_frame_payload[0], &per_ch_rate, 4);
    g_frame_payload[4] = ch_mask;
    g_frame_payload[5] = atten_code;
    g_frame_payload[6] = flags;
    g_frame_payload[7] = trig_ch;
    g_frame_payload[8]  = (uint8_t)(want_samples & 0xFF);
    g_frame_payload[9]  = (uint8_t)((want_samples >> 8) & 0xFF);
    g_frame_payload[10] = (uint8_t)(pre_samples & 0xFF);
    g_frame_payload[11] = (uint8_t)((pre_samples >> 8) & 0xFF);
    g_frame_payload[12] = (uint8_t)(trig_mv & 0xFF);
    g_frame_payload[13] = (uint8_t)((trig_mv >> 8) & 0xFF);
    g_frame_payload[14] = (uint8_t)(vref_mv & 0xFF);
    g_frame_payload[15] = (uint8_t)((vref_mv >> 8) & 0xFF);

    send_frame(TYPE_SCOPE_DATA, g_frame_payload, payload_len);
    last_tx_ms = millis();

    if (run_mode == 2) g_cfg.run_mode = 0;
  }
}

static void process_command(const uint8_t cmd[8]) {
  switch (cmd[2]) {
    case CMD_PING_STATUS:
    case CMD_GET_CAPS:
      send_capabilities();
      break;
    case CMD_SCOPE_CFG:
      g_cfg.rate_code = min<uint8_t>(cmd[3], 6);
      g_cfg.channel_mask = (cmd[4] & 0x07) ? (cmd[4] & 0x07) : 0x01;
      g_cfg.atten_code = min<uint8_t>(cmd[5], 3);
      g_cfg.depth_code = min<uint8_t>(cmd[6], 3);
      g_cfg.adc_dirty = true;
      break;
    case CMD_SCOPE_TRIG:
      g_cfg.trig_mode = cmd[3] & 0x03;
      g_cfg.trig_edge = (cmd[3] >> 4) & 0x01;
      g_cfg.trig_ch   = min<uint8_t>((cmd[3] >> 6) & 0x03, 2);
      g_cfg.trig_level_mv = (uint16_t)cmd[4] | ((uint16_t)cmd[5] << 8);
      g_cfg.pre_trig_pct = min<uint8_t>(cmd[6], 75);
      break;
    case CMD_SCOPE_RUN:
      g_cfg.run_mode = min<uint8_t>(cmd[3], 2);
      g_cfg.target_fps = constrain(cmd[4], 1, 30);
      g_cfg.test_gen_code = min<uint8_t>(cmd[5], 4);
      g_cfg.cali_enabled = (cmd[6] & 0x01) != 0;
      apply_test_generator(g_cfg.test_gen_code);
      break;
    case CMD_BOOTLOADER:
      if (cmd[3] == 0xB0 && cmd[4] == 0x07 && cmd[5] == 0x10 && cmd[6] == 0xAD) {
        stop_adc();
        esp_restart();
      }
      break;
    default:
      break;
  }
}

void setup() {
  Serial.begin(921600);
  g_serial_mutex = xSemaphoreCreateMutex();
  build_cali_lut(3);
  apply_test_generator(g_cfg.test_gen_code);

  xTaskCreate(scope_capture_task, "scope_adc", 6144, nullptr, 4, nullptr);
}

void loop() {
  static uint8_t rx_buf[8];
  static uint8_t rx_idx = 0;

  while (Serial.available() > 0) {
    uint8_t b = (uint8_t)Serial.read();
    if (rx_idx == 0) {
      if (b == SYNC_0) rx_buf[rx_idx++] = b;
    } else if (rx_idx == 1) {
      if (b == SYNC_1) rx_buf[rx_idx++] = b;
      else rx_idx = (b == SYNC_0) ? 1 : 0;
    } else {
      rx_buf[rx_idx++] = b;
      if (rx_idx == 8) {
        if (compute_xor(rx_buf, 7) == rx_buf[7]) {
          process_command(rx_buf);
        }
        rx_idx = 0;
      }
    }
  }
  vTaskDelay(pdMS_TO_TICKS(1));
}
