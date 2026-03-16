#!/usr/bin/env python3
"""Analyze touchpad logger output to find good activation parameters."""

import sys
import re

gestures = []
current_label = None
current_events = []
finger_down = False

# Parse the log
lines = open(sys.argv[1] if len(sys.argv) > 1 else "/dev/stdin").readlines()

for line in lines:
    line = line.strip()
    if not line or line.startswith("#"):
        if "LABEL:" in line:
            current_label = line.split("LABEL:")[-1].strip()
        elif "--- gesture" in line:
            if current_events:
                gestures.append((current_label or "unlabeled", current_events))
            current_events = []
            current_label = None
        continue

    parts = [p.strip() for p in line.split(",")]
    if len(parts) < 2:
        continue

    try:
        ts = float(parts[0])
    except ValueError:
        continue

    event = parts[1].strip()

    if event == "FINGER_DOWN":
        finger_down = True
        current_events.append({"ts": ts, "event": "DOWN"})
    elif event == "FINGER_UP":
        finger_down = False
        current_events.append({"ts": ts, "event": "UP"})
    elif event == "TOUCH":
        x, y = int(parts[2]), int(parts[3])
        current_events.append({"ts": ts, "event": "TOUCH", "x": x, "y": y})
    elif event == "MOVE":
        x, y = int(parts[2]), int(parts[3])
        dx, dy = int(parts[4]), int(parts[5])
        disp = float(parts[6])
        current_events.append({"ts": ts, "event": "MOVE", "x": x, "y": y, "dx": dx, "dy": dy, "disp": disp})

# Don't forget the last gesture
if current_events:
    gestures.append((current_label or "unlabeled", current_events))

print(f"{'='*70}")
print(f"TOUCHPAD DATA ANALYSIS")
print(f"{'='*70}")
print(f"Total gestures: {len(gestures)}")
print()

for category in ["intentional", "accidental", "unlabeled"]:
    cat_gestures = [(l, e) for l, e in gestures if category in (l or "").lower() or (category == "unlabeled" and l is None)]
    if not cat_gestures:
        continue

    print(f"\n{'='*70}")
    print(f"  {category.upper()} gestures ({len(cat_gestures)})")
    print(f"{'='*70}")

    for i, (label, events) in enumerate(cat_gestures):
        # Split into finger-down sessions
        sessions = []
        current_session = []
        for ev in events:
            if ev["event"] == "DOWN":
                current_session = []
            elif ev["event"] == "UP":
                if current_session:
                    sessions.append(current_session)
                current_session = []
            elif ev["event"] in ("TOUCH", "MOVE"):
                current_session.append(ev)
        if current_session:
            sessions.append(current_session)

        for si, session in enumerate(sessions):
            moves = [e for e in session if e["event"] == "MOVE"]
            if not moves and len(session) <= 1:
                duration = 0
                if len(events) >= 2:
                    downs = [e for e in events if e["event"] == "DOWN"]
                    ups = [e for e in events if e["event"] == "UP"]
                    if downs and ups:
                        duration = ups[0]["ts"] - downs[0]["ts"]
                print(f"\n  Gesture {i+1} session {si+1}: TAP (no movement, {duration:.0f}ms)")
                continue

            displacements = [m["disp"] for m in moves]
            duration = moves[-1]["ts"] - session[0]["ts"] if len(session) > 1 else 0
            total_move_count = len(moves)

            print(f"\n  Gesture {i+1} session {si+1}: {total_move_count} moves over {duration:.0f}ms")
            if displacements:
                print(f"    displacement/sample: min={min(displacements):.1f}  median={sorted(displacements)[len(displacements)//2]:.1f}  max={max(displacements):.1f}  mean={sum(displacements)/len(displacements):.1f}")

                # Simulate rolling window at different thresholds
                print(f"    --- Rolling window simulation (window=200ms, ratio=90%) ---")
                for threshold in [1, 2, 3, 5, 8]:
                    # Simulate at native report rate (~7ms)
                    all_samples = []
                    for ev in session:
                        if ev["event"] == "MOVE":
                            all_samples.append(ev["disp"] >= threshold)
                        elif ev["event"] == "TOUCH":
                            all_samples.append(False)

                    # Window size at native rate (~7ms per sample) for 200ms window
                    window_sz = max(1, 200 // 7)
                    activated = False
                    activation_sample = None
                    for j in range(len(all_samples)):
                        start = max(0, j - window_sz + 1)
                        window = all_samples[start:j+1]
                        if len(window) >= window_sz:
                            ratio = sum(window) / len(window)
                            if ratio >= 0.9 and not activated:
                                activated = True
                                activation_sample = j
                                break

                    if activated and activation_sample is not None:
                        activation_time = session[min(activation_sample, len(session)-1)]["ts"] - session[0]["ts"]
                        print(f"      threshold={threshold}: ACTIVATED at sample {activation_sample} ({activation_time:.0f}ms into gesture)")
                    else:
                        print(f"      threshold={threshold}: never activated")

print(f"\n{'='*70}")
print("RECOMMENDATION")
print(f"{'='*70}")
print("Look for a threshold where intentional gestures activate quickly")
print("but accidental ones never activate (or activate very late).")
