/*
 * SPDX-License-Identifier: Apache-2.0
 * Copyright (c) 2026 Kari Vierimaa, Kempele
 *
 * HotWire Oscilloscope — 4-Channel 12-bit GDMA ADC Oscilloscope Firmware
 * Target: Seeed Studio XIAO ESP32-S3
 *
 * Architecture:
 * - Core 1: GDMA Continuous ADC (`esp_adc/adc_continuous.h`) + O(1) eFuse Curve-Fitting
 *           4096-entry mV LUT + Pre-trigger Ring Buffer + Rising/Falling Edge Trigger Engine
 * - Core 0: USB CDC Binary Protocol Demuxer (`0xAA 0x55`) & Frame Transmitter
 * - Turbo / Overclock Mode (>83.3 kSPS up to 1 MSPS):
 *     Initializes GDMA via ESP-IDF `adc_continuous`, then directly tunes
 *     `APB_SARADC.ctrl2.timer_target` and `APB_SARADC.ctrl.sar_clk_div`.
 * - Test Signal Generator: LEDC hardware PWM on D6 (GPIO43) at 1 / 5 / 10 / 50 kHz.
 *
 * Arduino IDE Settings (arduino-esp32 3.x / ESP-IDF 5.x):
 *   - Board: "XIAO_ESP32S3"
 *   - USB CDC On Boot: "Enabled"
 *   - CPU Frequency: "240MHz (WiFi)"
 */

#include <Arduino.h>
#include <string.h>
#include "esp_adc/adc_continuous.h"
#include "esp_adc/adc_cali.h"
#include "esp_adc/adc_cali_scheme.h"
#include "soc/apb_saradc_struct.h"
#include "soc/rtc_cntl_reg.h"
#include "esp_system.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/semphr.h"

#ifndef CONFIG_IDF_TARGET_ESP32S3
  #error "This sketch targets Seeed Studio XIAO ESP32-S3."
#endif

#define HOTWIRE_FW_VERSION "1.0.0-scope"
#define HOTWIRE_BOARD_NAME "XIAO-ESP32-S3"

// XIAO ESP32-S3 Analog Pins: D0=GPIO1(CH0), D1=GPIO2(CH1), D2=GPIO3(CH2), D3=GPIO4(CH3)
static const adc_channel_t SCOPE_ADC_CHANNELS[4] = {
  ADC_CHANNEL_0, ADC_CHANNEL_1, ADC_CHANNEL_2, ADC_CHANNEL_3
};
static const int TEST_GEN_PIN = 43; // D6 on XIAO ESP32-S3

// Binary protocol constants
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
static const uint8_t FLAG_OVERCLOCKED   = 0x02;
static const uint8_t FLAG_CALIBRATED_MV = 0x04;
static const uint8_t FLAG_ADC_SATURATED = 0x08;

static const uint32_t RATE_TABLE[7]   = { 10000, 20000, 50000, 83333, 200000, 500000, 1000000 };
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
  uint8_t  trig_mode;       // 0=Auto, 1=Normal, 2=Single, 3=FreeRun
  uint8_t  trig_edge;       // 0=Rising, 1=Falling
  uint8_t  trig_ch;         // 0..3
  uint16_t trig_level_mv;
  uint8_t  pre_trig_pct;    // 0..75
  uint8_t  run_mode;        // 0=Stop, 1=Continuous, 2=Single
  uint8_t  target_fps;      // 1..30
  uint8_t  test_gen_code;   // 0=Off, 1=1k, 2=5k, 3=10k, 4=50k
  bool     cali_enabled;
  bool     adc_dirty;
};

static volatile ScopeConfig g_cfg = {
  3, 0x03, 3, 1,
  0, 0, 0, 1550, 50,
  0, 20, 1, true, true
};

static SemaphoreHandle_t g_serial_mutex = nullptr;
static adc_continuous_handle_t g_adc_handle = nullptr;
static bool g_adc_running = false;

// O(1) eFuse calibration lookup table: raw 12-bit (0..4095) -> calibrated mV
static uint16_t g_cali_lut[4096];
static uint8_t  g_cali_lut_atten = 0xFF;

// DMA read scratch & per-channel ring buffers (up to 4096 samples * 2 for pre+post trigger)
static constexpr size_t DMA_READ_BYTES = 4096;
static uint8_t  g_dma_buf[DMA_READ_BYTES];
static constexpr size_t RING_CAP = 8192;
static uint16_t g_ring[4][RING_CAP];
static uint16_t g_tx_samples[4 * 4096];

static uint8_t compute_xor(const uint8_t* data, size_t len) {
  uint8_t x = 0;
  for (size_t i = 0; i < len; ++i) x ^= data[i];
  return x;
}

static void send_frame(uint8_t type_id, const uint8_t* payload, uint16_t len) {
  uint8_t hdr[5] = {
    SYNC_0, SYNC_1, type_id,
    (uint8_t)(len & 0xFF), (uint8_t)((len >> 8) & 0xFF)
  };
  uint8_t chk = compute_xor(hdr, 5) ^ compute_xor(payload, len);

  if (g_serial_mutex) xSemaphoreTake(g_serial_mutex, portMAX_DELAY);
  Serial.write(hdr, 5);
  if (len > 0) Serial.write(payload, len);
  Serial.write(&chk, 1);
  if (g_serial_mutex) xSemaphoreGive(g_serial_mutex);
}

static void send_capabilities() {
  const char* json =
    "{\"board\":\"" HOTWIRE_BOARD_NAME "\","
    "\"firmware\":\"" HOTWIRE_FW_VERSION "\","
    "\"scope\":true,\"max_channels\":4,\"max_rate_hz\":1000000,\"adc_bits\":12}";
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

  if (has_cali && cali) {
    adc_cali_delete_scheme_curve_fitting(cali);
  }
  g_cali_lut_atten = atten_code;
}

static void apply_test_generator(uint8_t code) {
  static const uint32_t freqs[5] = { 0, 1000, 5000, 10000, 50000 };
  if (code > 4) code = 0;
  if (code == 0) {
    ledcDetach(TEST_GEN_PIN);
    pinMode(TEST_GEN_PIN, INPUT);
  } else {
    ledcAttach(TEST_GEN_PIN, freqs[code], 8);
    ledcWrite(TEST_GEN_PIN, 128); // 50% duty square wave
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

  if ((ch_mask & 0x0F) == 0) ch_mask = 0x01;
  uint32_t target_rate = RATE_TABLE[min<uint8_t>(rate_code, 6)];
  // ESP-IDF adc_continuous caps sample_freq_hz at SOC_ADC_SAMPLE_FREQ_THRES_HIGH (83333 Hz).
  // For Turbo/OC rates (>83333 Hz), init cleanly at 83333 Hz and then tune hardware dividers.
  uint32_t driver_rate = min<uint32_t>(target_rate, 83333);

  adc_continuous_handle_cfg_t handle_cfg = {};
  handle_cfg.max_store_buf_size = 16384;
  handle_cfg.conv_frame_size = DMA_READ_BYTES;
  if (adc_continuous_new_handle(&handle_cfg, &g_adc_handle) != ESP_OK) {
    g_adc_handle = nullptr;
    return;
  }

  adc_digi_pattern_config_t patterns[4] = {};
  uint8_t pat_cnt = 0;
  for (uint8_t ch = 0; ch < 4; ++ch) {
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
  dig_cfg.sample_freq_hz = driver_rate;
  dig_cfg.conv_mode = ADC_CONV_SINGLE_UNIT_1;
  dig_cfg.format = ADC_DIGI_OUTPUT_FORMAT_TYPE2;

  if (adc_continuous_config(g_adc_handle, &dig_cfg) != ESP_OK) {
    stop_adc();
    return;
  }

  if (adc_continuous_start(g_adc_handle) == ESP_OK) {
    g_adc_running = true;

    // Turbo / Overclock direct register tuning on ESP32-S3 APB_SARADC
    // Base digital controller clock is 5 MHz -> timer_target = 5,000,000 / target_rate
    if (target_rate > 83333) {
      if (target_rate >= 1000000) {
        APB_SARADC.ctrl.sar_clk_div = 2;       // Overclock SAR clock for 1 MSPS burst
        APB_SARADC.ctrl2.timer_target = 5;
      } else if (target_rate >= 500000) {
        APB_SARADC.ctrl.sar_clk_div = 4;       // Moderate OC for 500 kSPS
        APB_SARADC.ctrl2.timer_target = 10;
      } else {
        // 200 kSPS: within 5 MHz / 25 cycles theoretical SAR limit
        APB_SARADC.ctrl2.timer_target = 25;
      }
    }
  }
}

// Core 1 Task: Continuous GDMA Acquisition, Triggering & Packetizing
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

    uint8_t  ch_mask = g_cfg.channel_mask & 0x0F;
    if (ch_mask == 0) ch_mask = 0x01;
    uint8_t  active_cnt = __builtin_popcount(ch_mask);
    uint16_t want_samples = DEPTH_TABLE[min<uint8_t>(g_cfg.depth_code, 3)];
    uint8_t  trig_mode = g_cfg.trig_mode;
    uint8_t  trig_edge = g_cfg.trig_edge;
    uint8_t  trig_ch = g_cfg.trig_ch & 0x03;
    if ((ch_mask & (1 << trig_ch)) == 0) {
      for (uint8_t c = 0; c < 4; ++c) {
        if (ch_mask & (1 << c)) { trig_ch = c; break; }
      }
    }
    uint16_t trig_mv = g_cfg.trig_level_mv;
    uint16_t pre_samples = (uint16_t)(((uint32_t)want_samples * min<uint8_t>(g_cfg.pre_trig_pct, 75)) / 100U);
    uint16_t post_samples = want_samples - pre_samples;
    bool     cali_en = g_cfg.cali_enabled;

    // Frame rate pacing
    uint32_t min_interval_ms = 1000U / max<uint8_t>(g_cfg.target_fps, 1);
    uint32_t now_ms = millis();
    if (run_mode == 1 && (now_ms - last_tx_ms) < min_interval_ms) {
      vTaskDelay(pdMS_TO_TICKS(2));
      continue;
    }

    // Acquire samples from GDMA into ring buffer until triggered (or auto-timeout)
    size_t write_idx[4] = { 0, 0, 0, 0 };
    size_t total_trig_ch = 0;
    bool triggered = false;
    size_t trig_ring_pos = 0;
    size_t post_collected = 0;
    bool saturated = false;

    uint32_t acq_start_ms = millis();
    uint32_t timeout_ms = (trig_mode == 1 || run_mode == 2) ? 1200 : 120;
    uint16_t prev_trig_val = trig_mv;
    bool has_prev = false;

    while (g_cfg.run_mode != 0 && !g_cfg.adc_dirty) {
      uint32_t ret_num = 0;
      esp_err_t err = adc_continuous_read(g_adc_handle, g_dma_buf, DMA_READ_BYTES, &ret_num, 20);
      if (err == ESP_OK && ret_num > 0) {
        for (uint32_t i = 0; i + SOC_ADC_DIGI_RESULT_BYTES <= ret_num; i += SOC_ADC_DIGI_RESULT_BYTES) {
          const adc_digi_output_data_t* p = (const adc_digi_output_data_t*)&g_dma_buf[i];
          uint8_t ch = p->type2.channel;
          if (ch >= 4 || (ch_mask & (1 << ch)) == 0) continue;

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
                // Free Run: trigger immediately once pre-trigger is filled
                if (total_trig_ch >= pre_samples) {
                  triggered = true;
                  trig_ring_pos = pos;
                }
              } else if (has_prev && total_trig_ch >= pre_samples) {
                bool crossed = (trig_edge == 0)
                  ? (prev_trig_val < trig_mv && mv >= trig_mv)
                  : (prev_trig_val > trig_mv && mv <= trig_mv);
                if (crossed) {
                  triggered = true;
                  trig_ring_pos = pos;
                }
              }
              prev_trig_val = mv;
              has_prev = true;
            } else {
              post_collected++;
              if (post_collected >= post_samples) break;
            }
          }
        }
      }

      if (triggered && post_collected >= post_samples) break;

      if (!triggered && (millis() - acq_start_ms) >= timeout_ms) {
        if (trig_mode == 0) {
          // Auto mode fallback: force capture after timeout
          if (total_trig_ch >= want_samples) {
            trig_ring_pos = (write_idx[trig_ch] - post_samples) & (RING_CAP - 1);
            break;
          }
        } else if (trig_mode == 1) {
          // Normal mode: keep waiting but yield
          acq_start_ms = millis();
          vTaskDelay(pdMS_TO_TICKS(5));
        } else {
          break;
        }
      }
    }

    if (g_cfg.run_mode == 0 || g_cfg.adc_dirty) continue;

    // Extract planar frame
    uint32_t total_rate = RATE_TABLE[min<uint8_t>(g_cfg.rate_code, 6)];
    uint32_t per_ch_rate = max<uint32_t>(total_rate / active_cnt, 1000);
    uint8_t  atten_code = min<uint8_t>(g_cfg.atten_code, 3);
    uint16_t vref_mv = VREF_TABLE[atten_code];

    uint8_t flags = 0;
    if (triggered && trig_mode != 3) flags |= FLAG_TRIGGERED;
    if (total_rate > 83333) flags |= FLAG_OVERCLOCKED;
    if (cali_en) flags |= FLAG_CALIBRATED_MV;
    if (saturated) flags |= FLAG_ADC_SATURATED;

    size_t start_pos = (trig_ring_pos + RING_CAP - pre_samples) & (RING_CAP - 1);
    size_t out_idx = 0;
    for (uint8_t ch = 0; ch < 4; ++ch) {
      if ((ch_mask & (1 << ch)) == 0) continue;
      for (uint16_t k = 0; k < want_samples; ++k) {
        g_tx_samples[out_idx++] = g_ring[ch][(start_pos + k) & (RING_CAP - 1)];
      }
    }

    // Build payload header (16 bytes) + planar samples
    uint16_t payload_len = 16 + (uint16_t)(out_idx * 2);
    static uint8_t frame_payload[16 + 4 * 4096 * 2];
    memcpy(&frame_payload[0], &per_ch_rate, 4);
    frame_payload[4] = ch_mask;
    frame_payload[5] = atten_code;
    frame_payload[6] = flags;
    frame_payload[7] = trig_ch;
    frame_payload[8]  = (uint8_t)(want_samples & 0xFF);
    frame_payload[9]  = (uint8_t)((want_samples >> 8) & 0xFF);
    frame_payload[10] = (uint8_t)(pre_samples & 0xFF);
    frame_payload[11] = (uint8_t)((pre_samples >> 8) & 0xFF);
    frame_payload[12] = (uint8_t)(trig_mv & 0xFF);
    frame_payload[13] = (uint8_t)((trig_mv >> 8) & 0xFF);
    frame_payload[14] = (uint8_t)(vref_mv & 0xFF);
    frame_payload[15] = (uint8_t)((vref_mv >> 8) & 0xFF);
    memcpy(&frame_payload[16], g_tx_samples, out_idx * 2);

    send_frame(TYPE_SCOPE_DATA, frame_payload, payload_len);
    last_tx_ms = millis();

    if (run_mode == 2) {
      g_cfg.run_mode = 0; // Single-shot complete
    }
  }
}

// Core 0: Binary Command Parser
static void process_command(const uint8_t cmd[8]) {
  switch (cmd[2]) {
    case CMD_PING_STATUS:
    case CMD_GET_CAPS:
      send_capabilities();
      break;
    case CMD_SCOPE_CFG:
      g_cfg.rate_code = min<uint8_t>(cmd[3], 6);
      g_cfg.channel_mask = (cmd[4] & 0x0F) ? (cmd[4] & 0x0F) : 0x01;
      g_cfg.atten_code = min<uint8_t>(cmd[5], 3);
      g_cfg.depth_code = min<uint8_t>(cmd[6], 3);
      g_cfg.adc_dirty = true;
      break;
    case CMD_SCOPE_TRIG:
      g_cfg.trig_mode = cmd[3] & 0x03;
      g_cfg.trig_edge = (cmd[3] >> 4) & 0x01;
      g_cfg.trig_ch   = (cmd[3] >> 6) & 0x03;
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
        REG_WRITE(RTC_CNTL_OPTION1_REG, RTC_CNTL_FORCE_DOWNLOAD_BOOT);
        esp_restart();
      }
      break;
    default:
      break;
  }
}

void setup() {
  Serial.begin(921600);
  Serial.setTxBufferSize(8192);
  g_serial_mutex = xSemaphoreCreateMutex();
  build_cali_lut(3);
  apply_test_generator(g_cfg.test_gen_code);

  // Pin GDMA ADC acquisition + triggering to Core 1
  xTaskCreatePinnedToCore(scope_capture_task, "scope_adc", 8192, nullptr, 5, nullptr, 1);
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
