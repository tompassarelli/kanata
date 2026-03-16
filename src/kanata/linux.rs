#![cfg_attr(
    feature = "simulated_output",
    allow(dead_code, unused_imports, unused_variables, unused_mut)
)]

use anyhow::{Result, anyhow, bail};
use evdev::{EventSummary, InputEvent, RelativeAxisCode};
use log::info;
use parking_lot::Mutex;
use std::convert::TryFrom;
use std::sync::Arc;
use std::sync::mpsc::SyncSender as Sender;

use super::*;

impl Kanata {
    /// Enter an infinite loop that listens for OS key events and sends them to the processing
    /// thread.
    pub fn event_loop(kanata: Arc<Mutex<Self>>, tx: Sender<KeyEvent>) -> Result<()> {
        info!("entering the event loop");

        let k = kanata.lock();
        let allow_hardware_repeat = k.allow_hardware_repeat;
        let mouse_movement_key = k.mouse_movement_key.clone();
        let mut kbd_in = match KbdIn::new(
            &k.kbd_in_paths,
            k.continue_if_no_devices,
            k.include_names.clone(),
            k.exclude_names.clone(),
            k.device_detect_mode,
        ) {
            Ok(kbd_in) => kbd_in,
            Err(e) => {
                bail!("failed to open keyboard device(s): {}", e)
            }
        };

        // Start touchpad monitoring thread if configured.
        if let Some(ref tp_dev) = k.touchpad_dev_path {
            let tp_vk_name = k
                .touchpad_virtual_key
                .clone()
                .expect("parser validates both touchpad options are set together");
            let tp_vk_index = match k.virtual_keys.get(&tp_vk_name) {
                Some(&idx) => idx as u16,
                None => {
                    bail!(
                        "linux-touchpad-virtual-key '{tp_vk_name}' not found in defvirtualkeys"
                    );
                }
            };
            let tp_threshold = k.touchpad_threshold;
            let tp_activation_time = k.touchpad_activation_time;
            let mut touchpad_in = match crate::oskbd::TouchpadIn::new(tp_dev, tp_threshold, tp_activation_time) {
                Ok(tp) => tp,
                Err(e) => {
                    bail!("failed to open touchpad device: {e}");
                }
            };
            let kanata_clone = kanata.clone();
            std::thread::Builder::new()
                .name("touchpad-monitor".into())
                .spawn(move || {
                    touchpad_monitor_loop(kanata_clone, &mut touchpad_in, tp_vk_index);
                })
                .expect("failed to spawn touchpad monitor thread");
        }

        // In some environments, this needs to be done after the input device grab otherwise it
        // does not work on kanata startup.
        Kanata::set_repeat_rate(k.x11_repeat_rate)?;
        drop(k);

        loop {
            let events = kbd_in.read().map_err(|e| anyhow!("failed read: {}", e))?;
            log::trace!("event count: {}\nevents:\n{events:?}", events.len());

            for in_event in events.iter().copied() {
                if let Some(ms_mvmt_key) = *mouse_movement_key.lock()
                    && let EventSummary::RelativeAxis(_, _, _) = in_event.destructure()
                {
                    let fake_event = KeyEvent::new(ms_mvmt_key, KeyValue::Tap);
                    if let Err(e) = tx.try_send(fake_event) {
                        bail!("failed to send on channel: {}", e)
                    }
                }

                let key_event = match KeyEvent::try_from(in_event) {
                    Ok(ev) => ev,
                    _ => {
                        // Pass-through non-key and non-scroll events
                        let mut kanata = kanata.lock();
                        #[cfg(not(feature = "simulated_output"))]
                        kanata
                            .kbd_out
                            .write_raw(in_event)
                            .map_err(|e| anyhow!("failed write: {}", e))?;
                        continue;
                    }
                };

                check_for_exit(&key_event);

                if key_event.value == KeyValue::Repeat && !allow_hardware_repeat {
                    continue;
                }

                if key_event.value == KeyValue::Tap {
                    // Scroll event for sure. Only scroll events produce Tap.
                    if !handle_scroll(&kanata, in_event, key_event.code, &events)? {
                        continue;
                    }
                }

                match key_event.value {
                    KeyValue::Release => {
                        PRESSED_KEYS.lock().remove(&key_event.code);
                    }
                    KeyValue::Press => {
                        PRESSED_KEYS.lock().insert(key_event.code);
                    }
                    _ => {}
                }

                // Handle normal keypresses.
                // Check if this keycode is mapped in the configuration.
                // If it hasn't been mapped, send it immediately.
                if !MAPPED_KEYS.lock().contains(&key_event.code) {
                    let mut kanata = kanata.lock();
                    #[cfg(not(feature = "simulated_output"))]
                    kanata
                        .kbd_out
                        .write_raw(in_event)
                        .map_err(|e| anyhow!("failed write: {}", e))?;
                    continue;
                };

                // Send key events to the processing loop
                if let Err(e) = tx.try_send(key_event) {
                    bail!("failed to send on channel: {}", e)
                }
            }
        }
    }

    pub fn check_release_non_physical_shift(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn set_repeat_rate(s: Option<KeyRepeatSettings>) -> Result<()> {
        if let Some(s) = s {
            log::info!(
                "Using xset to set X11 repeat delay to {} and repeat rate to {}",
                s.delay,
                s.rate,
            );
            let cmd_output = std::process::Command::new("xset")
                .args([
                    "r",
                    "rate",
                    s.delay.to_string().as_str(),
                    s.rate.to_string().as_str(),
                ])
                .output()
                .map_err(|e| {
                    log::error!("failed to run xset: {e:?}");
                    e
                })?;
            log::info!(
                "xset stdout: {}",
                String::from_utf8_lossy(&cmd_output.stdout)
            );
            log::info!(
                "xset stderr: {}",
                String::from_utf8_lossy(&cmd_output.stderr)
            );
        }
        Ok(())
    }
}

/// Returns true if the scroll event should be sent to the processing loop, otherwise returns
/// false.
fn handle_scroll(
    kanata: &Mutex<Kanata>,
    in_event: InputEvent,
    code: OsCode,
    all_events: &[InputEvent],
) -> Result<bool> {
    let direction: MWheelDirection = code.try_into().unwrap();
    let scroll_distance = in_event.value().unsigned_abs() as u16;
    match in_event.destructure() {
        EventSummary::RelativeAxis(_, axis_type, _) => {
            match axis_type {
                RelativeAxisCode::REL_WHEEL | RelativeAxisCode::REL_HWHEEL => {
                    if MAPPED_KEYS.lock().contains(&code) {
                        return Ok(true);
                    }
                    // If we just used `write_raw` here, some of the scrolls issued by kanata would be
                    // REL_WHEEL_HI_RES + REL_WHEEL and some just REL_WHEEL and an issue like this one
                    // would happen: https://github.com/jtroo/kanata/issues/395
                    //
                    // So to fix this case, we need to use `scroll` which will also send hi-res scrolls
                    // along normal scrolls.
                    //
                    // However, if this is a normal scroll event, it may be sent alongside a hi-res
                    // scroll event. In this scenario, the hi-res event should be used to call
                    // scroll, and not the normal event. Otherwise, too much scrolling will happen.
                    let mut kanata = kanata.lock();
                    if !all_events.iter().any(|ev| {
                        matches!(
                            ev.destructure(),
                            EventSummary::RelativeAxis(
                                _,
                                RelativeAxisCode::REL_WHEEL_HI_RES
                                    | RelativeAxisCode::REL_HWHEEL_HI_RES,
                                _
                            )
                        )
                    }) {
                        kanata
                            .kbd_out
                            .scroll(direction, scroll_distance * HI_RES_SCROLL_UNITS_IN_LO_RES)
                            .map_err(|e| anyhow!("failed write: {}", e))?;
                    }
                    Ok(false)
                }
                RelativeAxisCode::REL_WHEEL_HI_RES | RelativeAxisCode::REL_HWHEEL_HI_RES => {
                    if !MAPPED_KEYS.lock().contains(&code) {
                        // Passthrough if the scroll wheel event is not mapped
                        // in the configuration.
                        let mut kanata = kanata.lock();
                        kanata
                            .kbd_out
                            .scroll(direction, scroll_distance)
                            .map_err(|e| anyhow!("failed write: {}", e))?;
                    }
                    // Kanata will not handle high resolution scroll events for now.
                    // Full notch scrolling only.
                    Ok(false)
                }
                _ => unreachable!("expect to be handling a wheel event"),
            }
        }
        _ => unreachable!("expect to be handling a wheel event"),
    }
}

/// Runs in a dedicated thread. Blocks on the touchpad fd for contact state changes
/// and injects virtual key press/release into the kanata layout.
///
/// A separate thread is used rather than registering the touchpad fd in the existing
/// KbdIn mio::Poll because:
/// - KbdIn::read() returns Vec<InputEvent> and expects keyboard events; touchpad
///   events need fundamentally different processing (BTN_TOOL_FINGER, not key codes).
/// - The TCP server already establishes the pattern of locking the Kanata mutex and
///   calling handle_fakekey_action from a separate thread.
/// - Mixing the touchpad fd into KbdIn would require restructuring the read loop and
///   its return type, for minimal benefit over a dedicated blocking thread.
fn touchpad_monitor_loop(
    kanata: Arc<Mutex<Kanata>>,
    touchpad_in: &mut crate::oskbd::TouchpadIn,
    vk_index: u16,
) {
    use kanata_parser::cfg::FAKE_KEY_ROW;
    info!("touchpad monitor thread started");
    loop {
        match touchpad_in.read_contact_change() {
            Ok(Some(true)) => {
                log::debug!("touchpad: finger down, pressing virtual key");
                let mut k = kanata.lock();
                handle_fakekey_action(
                    FakeKeyAction::Press,
                    k.layout.bm(),
                    FAKE_KEY_ROW,
                    vk_index,
                );
            }
            Ok(Some(false)) => {
                log::debug!("touchpad: finger up, releasing virtual key");
                let mut k = kanata.lock();
                handle_fakekey_action(
                    FakeKeyAction::Release,
                    k.layout.bm(),
                    FAKE_KEY_ROW,
                    vk_index,
                );
            }
            Ok(None) => {}
            Err(e) => {
                log::error!("touchpad monitor error: {e}");
                break;
            }
        }
    }
    // Guarantee release if the thread exits while the touchpad was active.
    if touchpad_in.is_active {
        log::warn!("touchpad monitor exiting while active, releasing virtual key");
        let mut k = kanata.lock();
        handle_fakekey_action(
            FakeKeyAction::Release,
            k.layout.bm(),
            FAKE_KEY_ROW,
            vk_index,
        );
    }
    log::warn!("touchpad monitor thread exiting");
}
