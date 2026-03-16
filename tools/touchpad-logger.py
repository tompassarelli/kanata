#!/usr/bin/env python3
"""
Log touchpad ABS_X/ABS_Y events with displacement per sample.
Run with: sudo python3 tools/touchpad-logger.py [device_path]

Produces CSV-style output:
  timestamp_ms, event, x, y, dx, dy, displacement

Label your gestures by pressing Enter between them and typing a label.
Ctrl+C to stop.
"""

import struct
import sys
import time
import os
import select
import math

# evdev event struct: time_sec(8) time_usec(8) type(2) code(2) value(4) = 24 bytes
EVENT_FORMAT = "llHHi"
EVENT_SIZE = struct.calcsize(EVENT_FORMAT)

EV_KEY = 0x01
EV_ABS = 0x03

ABS_X = 0x00
ABS_Y = 0x01
ABS_MT_POSITION_X = 0x35
ABS_MT_POSITION_Y = 0x36

BTN_TOOL_FINGER = 325

def main():
    dev_path = sys.argv[1] if len(sys.argv) > 1 else "/dev/input/by-path/platform-AMDI0010:03-event-mouse"

    fd = os.open(dev_path, os.O_RDONLY)
    print(f"# Logging touchpad events from {dev_path}")
    print(f"# Press Enter to insert a label marker, Ctrl+C to stop")
    print(f"# timestamp_ms, event, x, y, dx, dy, displacement")
    print()

    finger_down = False
    last_x = None
    last_y = None
    cur_x = None
    cur_y = None
    start_time = time.monotonic()
    gesture_num = 0

    try:
        while True:
            # Check for stdin (label input) without blocking
            if select.select([sys.stdin], [], [], 0)[0]:
                label = sys.stdin.readline().strip()
                if label:
                    print(f"# LABEL: {label}")
                else:
                    gesture_num += 1
                    print(f"# --- gesture {gesture_num} ---")
                sys.stdout.flush()

            # Read evdev events
            ready, _, _ = select.select([fd], [], [], 0.001)
            if not ready:
                continue

            data = os.read(fd, EVENT_SIZE * 64)
            for offset in range(0, len(data), EVENT_SIZE):
                if offset + EVENT_SIZE > len(data):
                    break
                _, _, ev_type, ev_code, ev_value = struct.unpack_from(EVENT_FORMAT, data, offset)
                ts = (time.monotonic() - start_time) * 1000  # ms since start

                if ev_type == EV_KEY and ev_code == BTN_TOOL_FINGER:
                    if ev_value != 0:
                        finger_down = True
                        last_x = None
                        last_y = None
                        cur_x = None
                        cur_y = None
                        print(f"{ts:10.1f}, FINGER_DOWN, , , , ,")
                    else:
                        finger_down = False
                        print(f"{ts:10.1f}, FINGER_UP, , , , ,")
                    sys.stdout.flush()

                elif ev_type == EV_ABS and finger_down:
                    if ev_code in (ABS_X, ABS_MT_POSITION_X):
                        cur_x = ev_value
                    elif ev_code in (ABS_Y, ABS_MT_POSITION_Y):
                        cur_y = ev_value

                # On SYN_REPORT (type=0, code=0), emit a sample if we have position
                elif ev_type == 0 and ev_code == 0 and finger_down and (cur_x is not None or cur_y is not None):
                    x = cur_x if cur_x is not None else (last_x or 0)
                    y = cur_y if cur_y is not None else (last_y or 0)

                    if last_x is not None and last_y is not None:
                        dx = x - last_x
                        dy = y - last_y
                        disp = math.sqrt(dx*dx + dy*dy)
                        print(f"{ts:10.1f}, MOVE, {x}, {y}, {dx}, {dy}, {disp:.1f}")
                    else:
                        print(f"{ts:10.1f}, TOUCH, {x}, {y}, 0, 0, 0.0")

                    last_x = x
                    last_y = y
                    cur_x = None
                    cur_y = None
                    sys.stdout.flush()

    except KeyboardInterrupt:
        print("\n# Done")
    finally:
        os.close(fd)

if __name__ == "__main__":
    main()
