use leptos::prelude::*;

use crate::components::bottom_drawer::BottomDrawer;
use crate::components::measurements_bar::MeasurementsBar;
use crate::components::scope_canvas::ScopeCanvas;
use crate::components::scope_control_bar::ScopeControlBar;
use crate::components::top_toolbar::TopToolbar;
use crate::protocol::codec::{build_scope_cfg_cmd, build_scope_run_cmd, build_scope_trig_cmd};
use crate::protocol::demuxer::ProtocolDemuxer;
use crate::protocol::types::{
    BoardCapabilitiesFrame, DecodedFrame, PersistenceMode, ScopeFrame, SCOPE_EDGE_RISING,
    SCOPE_RUN_CONTINUOUS, SCOPE_RUN_SINGLE, SCOPE_RUN_STOP, SCOPE_TRIG_AUTO,
};
use crate::transport::serial::{ConnectionStatus, SerialTransport, TransportSnapshot};

fn perf_now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

#[component]
pub fn App() -> impl IntoView {
    let serial_transport = StoredValue::new_local(SerialTransport::new());

    let transport = RwSignal::new(serial_transport.with_value(|t| t.snapshot()));
    let caps = RwSignal::new(None::<BoardCapabilitiesFrame>);

    // Acquisition & trigger settings
    let running = RwSignal::new(false);
    let rate_code = RwSignal::new(3u8); // 83.3 kSPS spec default
    let channel_mask = RwSignal::new(0x03u8); // CH1 + CH2
    let atten_code = RwSignal::new(3u8); // 12 dB (0-3.1V)
    let depth_code = RwSignal::new(1u8); // 1024 samples/ch
    let trig_mode = RwSignal::new(SCOPE_TRIG_AUTO);
    let trig_edge = RwSignal::new(SCOPE_EDGE_RISING);
    let trig_channel = RwSignal::new(0u8);
    let trig_level_mv = RwSignal::new(1550u16);
    let pre_trigger_pct = RwSignal::new(50u8);
    let test_gen_code = RwSignal::new(1u8); // 1 kHz test square wave
    let target_fps = RwSignal::new(20u8);

    // View & analysis controls
    let show_fft = RwSignal::new(false);
    let fft_channel = RwSignal::new(0u8);
    let show_cursors = RwSignal::new(false);
    let cursor_a_norm = RwSignal::new(0.25f64);
    let cursor_b_norm = RwSignal::new(0.75f64);
    let zoom = RwSignal::new(1.0f64);
    let pan_norm = RwSignal::new(0.0f64);
    let persistence_mode = RwSignal::new(PersistenceMode::Off);
    let persist_clear_ver = RwSignal::new(0u64);

    // Telemetry & frames
    let latest_frame = RwSignal::new(None::<ScopeFrame>);
    let frame_version = RwSignal::new(0u64);
    let fps = RwSignal::new(0.0f64);
    let drawer_open = RwSignal::new(false);

    let last_frame_ts = StoredValue::new(0.0f64);
    let demuxer = StoredValue::new_local(ProtocolDemuxer::new());

    let send_cfg = move || {
        let cmd = build_scope_cfg_cmd(
            rate_code.get_untracked(),
            channel_mask.get_untracked(),
            atten_code.get_untracked(),
            depth_code.get_untracked(),
        );
        serial_transport.with_value(|t| t.send_bytes(&cmd));
    };

    let send_trig = move || {
        let cmd = build_scope_trig_cmd(
            trig_mode.get_untracked(),
            trig_edge.get_untracked(),
            trig_channel.get_untracked(),
            trig_level_mv.get_untracked(),
            pre_trigger_pct.get_untracked(),
        );
        serial_transport.with_value(|t| t.send_bytes(&cmd));
    };

    let send_run = move |mode: u8| {
        let cmd = build_scope_run_cmd(
            mode,
            target_fps.get_untracked(),
            test_gen_code.get_untracked(),
            true,
        );
        serial_transport.with_value(|t| t.send_bytes(&cmd));
    };

    // Initialize transport callbacks
    serial_transport.with_value(|st| {
        st.init(
            move |snap: TransportSnapshot| {
                let connected = snap.status == ConnectionStatus::Connected;
                transport.set(snap);
                if !connected {
                    running.set(false);
                    caps.set(None);
                    fps.set(0.0);
                    last_frame_ts.set_value(0.0);
                    demuxer.update_value(|d| d.reset());
                }
            },
            move |bytes: &[u8]| {
                let now = perf_now();
                demuxer.update_value(|d| {
                    d.feed(bytes, now, |frame| match frame {
                        DecodedFrame::Capabilities(c) => {
                            caps.set(Some(c));
                            // Push current config and start streaming upon connect
                            send_cfg();
                            send_trig();
                            running.set(true);
                            send_run(SCOPE_RUN_CONTINUOUS);
                        }
                        DecodedFrame::Scope(scope_frame) => {
                            let prev = last_frame_ts.get_value();
                            if prev > 0.0 {
                                let dt = now - prev;
                                if dt > 1.0 {
                                    let inst = 1000.0 / dt;
                                    fps.update(|f| *f = if *f == 0.0 { inst } else { *f * 0.8 + inst * 0.2 });
                                }
                            }
                            last_frame_ts.set_value(now);
                            latest_frame.set(Some(scope_frame));
                            frame_version.update(|v| *v = v.wrapping_add(1));
                        }
                    });
                });
            },
        );
    });

    let on_connect = Callback::new(move |_| {
        serial_transport.with_value(|t| t.connect_usb());
    });
    let on_disconnect = Callback::new(move |_| {
        running.set(false);
        send_run(SCOPE_RUN_STOP);
        serial_transport.with_value(|t| t.disconnect());
    });
    let on_start_demo = Callback::new(move |_| {
        serial_transport.with_value(|t| t.start_demo());
    });
    let on_toggle_run = Callback::new(move |_| {
        let next = !running.get_untracked();
        running.set(next);
        send_run(if next { SCOPE_RUN_CONTINUOUS } else { SCOPE_RUN_STOP });
    });
    let on_single_shot = Callback::new(move |_| {
        running.set(false);
        send_run(SCOPE_RUN_SINGLE);
    });
    let on_toggle_drawer = Callback::new(move |_| {
        drawer_open.update(|v| *v = !*v);
    });
    let on_close_drawer = Callback::new(move |_| drawer_open.set(false));
    let on_apply_cfg = Callback::new(move |_| {
        persist_clear_ver.update(|v| *v = v.wrapping_add(1));
        send_cfg();
    });
    let on_clear_persist = Callback::new(move |_| {
        persist_clear_ver.update(|v| *v = v.wrapping_add(1));
    });
    let on_apply_trig = Callback::new(move |_| send_trig());
    let on_apply_run = Callback::new(move |_| {
        send_run(if running.get_untracked() {
            SCOPE_RUN_CONTINUOUS
        } else {
            SCOPE_RUN_STOP
        });
    });
    let on_reset_view = Callback::new(move |_| {
        zoom.set(1.0);
        pan_norm.set(0.0);
    });

    view! {
        <div class="app-shell">
            <TopToolbar
                transport=transport
                caps=caps
                running=running
                fps=fps
                on_connect=on_connect
                on_disconnect=on_disconnect
                on_start_demo=on_start_demo
                on_toggle_run=on_toggle_run
                on_single_shot=on_single_shot
                drawer_open=drawer_open
                on_toggle_drawer=on_toggle_drawer
            />

            <ScopeControlBar
                rate_code=rate_code
                channel_mask=channel_mask
                atten_code=atten_code
                depth_code=depth_code
                trig_mode=trig_mode
                trig_edge=trig_edge
                trig_channel=trig_channel
                trig_level_mv=trig_level_mv
                test_gen_code=test_gen_code
                show_fft=show_fft
                show_cursors=show_cursors
                fft_channel=fft_channel
                persistence_mode=persistence_mode
                on_clear_persist=on_clear_persist
                on_apply_cfg=on_apply_cfg
                on_apply_trig=on_apply_trig
                on_apply_run=on_apply_run
                on_reset_view=on_reset_view
            />

            <main class="scope-main">
                <ScopeCanvas
                    latest_frame=latest_frame
                    frame_version=frame_version
                    trig_level_mv=trig_level_mv
                    trig_channel=trig_channel
                    show_fft=show_fft
                    fft_channel=fft_channel
                    show_cursors=show_cursors
                    cursor_a_norm=cursor_a_norm
                    cursor_b_norm=cursor_b_norm
                    zoom=zoom
                    pan_norm=pan_norm
                    persistence_mode=persistence_mode
                    persist_clear_ver=persist_clear_ver
                />
            </main>

            <MeasurementsBar
                latest_frame=latest_frame
                show_cursors=show_cursors
                cursor_a_norm=cursor_a_norm
                cursor_b_norm=cursor_b_norm
                zoom=zoom
                pan_norm=pan_norm
            />

            <BottomDrawer
                open=drawer_open
                on_close=on_close_drawer
                caps=caps
                serial_transport=serial_transport
            />
        </div>
    }
}
