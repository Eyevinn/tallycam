#!/usr/bin/env python3
"""Pair analysis of the torch probe (docs/GENLOCK.md, "Torch probe").

One phone switches its torch (`torch_probe` with mode `emit`, genlocked), a
second camera films the same white sheet (`torch_probe` with mode `watch`)
over the same seconds. Both write Documents/torch-probe/<time>-<model>/; copy
the two folders off the devices and run:

    python3 analyze.py EMITTER_FOLDER WATCHER_FOLDER

The iPhone's torch switches on frame boundaries (first run, GENLOCK.md), so
the emitter sees each switch as a whole frame. The watcher's rolling shutter
sees it inside a frame, at some line. Per switch, in TAI:

    light time = watcher stamp + watcher first-line offset + line x line time

and the light time is expected at a fixed point of the emitter's frame. So
the watcher's edge line is linear in (emitter's first switched frame stamp -
watcher stamp): the slope gives the watcher's line time, the intercept
"light time after the emitter's frame stamp - watcher's first-line offset",
and the scatter how steady the light is on the emitter's frame grid (plus
the two clocks' disagreement). The same fit against the command time instead
shows whether the frame or the command sets the light.

The watcher's frames must cover the frame phase: run it with genlock off and
a frame duration slightly off the emitter's (`frame_duration` with us 33400
against a 30 fps emitter slides one frame in about 17 s). A genlocked
watcher sees every edge at the same line, which measures steadiness but not
the line time.

Standard library only. `--selftest` checks the analysis on simulated data.
"""

import csv
import math
import os
import random
import sys

csv.field_size_limit(sys.maxsize)


def load_frames(folder):
    frames = []
    with open(os.path.join(folder, "frames.csv")) as f:
        for row in csv.DictReader(f):
            if "tai_ns" not in row:
                sys.exit(f"{folder}: frames.csv has no TAI stamps (app too old)")
            profile = [float(v) for v in row["profile"].split()]
            frames.append((int(row["tai_ns"]), row["synced"] == "1", profile))
    frames.sort(key=lambda f: f[0])
    return frames


def load_commands(folder):
    with open(os.path.join(folder, "commands.csv")) as f:
        return [(r["edge"], int(r["before_tai_ns"]), int(r["after_tai_ns"])) for r in csv.DictReader(f)]


def load_summary(folder):
    values = {}
    path = os.path.join(folder, "summary.txt")
    if os.path.exists(path):
        for line in open(path):
            for part in line.split():
                if "=" in part:
                    key, value = part.split("=", 1)
                    values[key] = value
    return values


def mean(values):
    return sum(values) / len(values) if values else 0.0


def levels(frames):
    means = sorted(mean(p) for _, _, p in frames)
    return means[len(means) * 5 // 100], means[min(len(means) - 1, len(means) * 95 // 100)]


def smooth(values, radius):
    prefix = [0.0]
    for v in values:
        prefix.append(prefix[-1] + v)
    out = []
    for i in range(len(values)):
        lo, hi = max(0, i - radius), min(len(values), i + radius + 1)
        out.append((prefix[hi] - prefix[lo]) / (hi - lo))
    return out


def first_crossing(values, threshold, rising):
    for i in range(1, len(values)):
        a, b = values[i - 1], values[i]
        if (rising and a < threshold <= b) or (not rising and a > threshold >= b):
            return i - 1 + (threshold - a) / (b - a)
    return None


def crossing(profile, dark, bright, edge):
    """The edge inside one frame, as in TorchProbeAnalysis.crossing: (line in
    readout order, width 10-90% in lines, reversed) or None."""
    contrast = bright - dark
    if contrast <= 0 or len(profile) < 32:
        return None
    level = [(v - dark) / contrast for v in smooth(profile, 4)]
    if min(level) >= 0.2 or max(level) <= 0.8:
        return None
    band = max(4, len(level) // 20)
    head, tail = mean(level[:band]), mean(level[-band:])
    head_dark = head < 0.2 and tail > 0.8
    head_bright = head > 0.8 and tail < 0.2
    if not (head_dark or head_bright):
        return None
    reversed_ = (edge == "on") != head_dark
    ordered = level[::-1] if reversed_ else level
    rising = edge == "on"
    mid = first_crossing(ordered, 0.5, rising)
    start = first_crossing(ordered, 0.1 if rising else 0.9, rising)
    end = first_crossing(ordered, 0.9 if rising else 0.1, rising)
    if mid is None or start is None or end is None:
        return None
    return mid, max(0.0, end - start), reversed_


def fit(points):
    """Least squares of line on x (ns), refitted once without 3-sigma
    outliers. Returns dict or None."""
    def line(pts):
        if len(pts) < 3:
            return None
        mx = mean([x for x, _ in pts])
        my = mean([y for _, y in pts])
        sxx = sum((x - mx) ** 2 for x, _ in pts)
        if sxx == 0:
            return None
        slope = sum((x - mx) * (y - my) for x, y in pts) / sxx
        return slope, my - slope * mx, sxx

    first = line(points)
    if first is None:
        return None
    slope, intercept, _ = first
    sigma = math.sqrt(mean([(y - slope * x - intercept) ** 2 for x, y in points]))
    kept = [(x, y) for x, y in points if abs(y - slope * x - intercept) <= max(3 * sigma, 1)]
    # As TorchProbeAnalysis.fit: keep the first line when the refit has too
    # few points.
    refit = line(kept) if len(kept) < len(points) else None
    if refit is None:
        kept = points
        refit = first
    slope, intercept, sxx = refit
    if slope == 0:
        # Every edge at the same line (a genlocked watcher): no line time.
        return None
    row_time = 1 / slope
    residuals = [(y - slope * x - intercept) * abs(row_time) for x, y in kept]
    rows = [y - slope * x - intercept for x, y in kept]
    slope_error = math.sqrt(sum(r * r for r in rows) / (len(kept) - 2) / sxx) if len(kept) > 2 else 0
    return {
        "row_time_ns": row_time,
        "row_time_error_ns": slope_error * row_time * row_time,
        "intercept_ns": intercept * row_time,
        "rms_ns": math.sqrt(mean([r * r for r in residuals])),
        "max_ns": max(abs(r) for r in residuals),
        "used": len(kept),
        "total": len(points),
    }


def analyze(emitter_frames, commands, watcher_frames, frame_ns):
    e_dark, e_bright = levels(emitter_frames)
    w_dark, w_bright = levels(watcher_frames)
    out = {"emitter_levels": (e_dark, e_bright), "watcher_levels": (w_dark, w_bright),
           "emitter_partial": 0, "events": [], "missed_emitter": 0, "missed_watcher": 0}
    e_contrast = e_bright - e_dark
    if e_contrast <= 0 or w_bright - w_dark <= 0:
        # A camera that never saw the torch change: nothing to pair.
        out["missed_emitter"] = len(commands)
        for edge in ("on", "off", "both"):
            out[f"fit_frame_{edge}"] = out[f"fit_command_{edge}"] = None
        return out
    for _, _, p in emitter_frames:
        n = [(v - e_dark) / e_contrast for v in p]
        if min(n) < 0.3 and max(n) > 0.7:
            out["emitter_partial"] += 1
    for edge, before, after in commands:
        command = before + (after - before) // 2
        want_lit = edge == "on"
        switched = None
        for tai, _, p in emitter_frames:
            if tai > command - 2 * frame_ns and ((mean(p) - e_dark) / e_contrast > 0.5) == want_lit:
                switched = tai
                break
        if switched is None or switched > command + 3 * frame_ns:
            out["missed_emitter"] += 1
            continue
        found = None
        for tai, _, p in watcher_frames:
            if switched - 3 * frame_ns <= tai <= switched + 2 * frame_ns:
                c = crossing(p, w_dark, w_bright, edge)
                if c:
                    found = (tai, c, len(p))
                    break
        if found is None:
            out["missed_watcher"] += 1
            continue
        tai, (line, width, reversed_), lines = found
        out["events"].append({"edge": edge, "command": command, "switched": switched,
                              "watcher_stamp": tai, "line": line, "width": width,
                              "reversed": reversed_, "lines": lines})
    for edge in ("on", "off", "both"):
        events = [e for e in out["events"] if edge == "both" or e["edge"] == edge]
        out[f"fit_frame_{edge}"] = fit([(e["switched"] - e["watcher_stamp"], e["line"]) for e in events])
        out[f"fit_command_{edge}"] = fit([(e["command"] - e["watcher_stamp"], e["line"]) for e in events])
    return out


def report(out):
    e, w = out["emitter_levels"], out["watcher_levels"]
    print(f"emitter luma {e[0]:.0f} to {e[1]:.0f}, frames with both states: {out['emitter_partial']}")
    print(f"watcher luma {w[0]:.0f} to {w[1]:.0f}")
    print(f"switches used {len(out['events'])}, missed at emitter {out['missed_emitter']}, "
          f"no edge inside a watcher frame {out['missed_watcher']}")
    if out["events"]:
        lines = out["events"][0]["lines"]
        reversed_ = sum(e["reversed"] for e in out["events"]) * 2 > len(out["events"])
        widths = sorted(e["width"] for e in out["events"])
        print(f"watcher: {lines} lines, readout from the buffer's {'last' if reversed_ else 'first'} line, "
              f"median edge width {widths[len(widths) // 2]:.0f} lines")
    for reference in ("frame", "command"):
        label = "emitter's first switched frame" if reference == "frame" else "torch command"
        print(f"\nagainst the {label}:")
        for edge in ("on", "off", "both"):
            f = out[f"fit_{reference}_{edge}"]
            if f is None:
                print(f"  {edge}: too few edges to fit")
                continue
            print(f"  {edge}: line {f['row_time_ns'] / 1e3:.3f} ± {f['row_time_error_ns'] / 1e3:.3f} us "
                  f"(readout {abs(f['row_time_ns']) * lines / 1e6:.2f} ms), "
                  f"light - watcher offset {f['intercept_ns'] / 1e6:+.3f} ms, "
                  f"scatter rms {f['rms_ns'] / 1e6:.3f} max {f['max_ns'] / 1e6:.3f} ms, {f['used']}/{f['total']}")
    print("\nThe smaller scatter tells which sets the light: the emitter's frame or the command.")


def selftest():
    random.seed(1)
    frame_ns = 33_333_333
    lines, line_ns, first_ns, shutter_ns = 720, 25_000, 1_500_000, 500_000
    light_after_frame_ns = 3_000_000
    switches = []
    commands = []
    t = 1_000_000_000
    for cycle in range(60):
        on = t + cycle * 600_000_000 + random.uniform(0, frame_ns)
        for edge, cmd in (("on", on), ("off", on + 300_000_000)):
            commands.append((edge, int(cmd), int(cmd) + 200_000))
            frame = math.ceil((cmd + 2_000_000) / frame_ns) * frame_ns
            switches.append((edge, frame, frame + light_after_frame_ns + random.gauss(0, 100_000)))
    emitter = []
    for k in range(int(40e9 / frame_ns)):
        tai = k * frame_ns
        state = False
        for edge, frame, _ in switches:
            if frame <= tai:
                state = edge == "on"
        emitter.append((tai, True, [180.0 if state else 30.0] * 64))

    def lit_fraction(a, b):
        total = 0.0
        on_at = None
        for edge, _, light in switches:
            if edge == "on":
                on_at = light
            elif on_at is not None:
                total += max(0.0, min(b, light) - max(a, on_at))
                on_at = None
        return total / (b - a)

    watcher = []
    watch_frame = 33_400_000
    for k in range(int(39e9 / watch_frame)):
        tai = 5_000_000 + k * watch_frame
        profile = []
        for line in range(lines):
            middle = tai + first_ns + line * line_ns
            profile.append(20 + 150 * lit_fraction(middle - shutter_ns / 2, middle + shutter_ns / 2))
        watcher.append((tai, True, profile))
    out = analyze(emitter, commands, watcher, frame_ns)
    f = out["fit_frame_both"]
    c = out["fit_command_both"]
    assert f and abs(f["row_time_ns"] - line_ns) < 3 * f["row_time_error_ns"] + 50, f
    assert abs(f["intercept_ns"] - (light_after_frame_ns - first_ns)) < 300_000, f
    assert f["rms_ns"] < 250_000, f
    assert c is None or c["rms_ns"] > 2 * f["rms_ns"], c
    report(out)
    print("\nselftest ok")


def main():
    if sys.argv[1:] == ["--selftest"]:
        selftest()
        return
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    emitter, watcher = sys.argv[1], sys.argv[2]
    summary = load_summary(emitter)
    frame_ns = float(summary.get("frame_us", "33333.333")) * 1000
    e_frames, w_frames = load_frames(emitter), load_frames(watcher)
    for name, frames in (("emitter", e_frames), ("watcher", w_frames)):
        unsynced = sum(1 for _, synced, _ in frames if not synced)
        if unsynced:
            print(f"warning: {unsynced} of {len(frames)} {name} frames without a synced clock")
    overlap = min(e_frames[-1][0], w_frames[-1][0]) - max(e_frames[0][0], w_frames[0][0])
    if overlap <= 0:
        sys.exit("the two recordings do not overlap in time")
    print(f"emitter {summary.get('model', '?')} {summary.get('format', '')}, "
          f"watcher {load_summary(watcher).get('model', '?')} {load_summary(watcher).get('format', '')}, "
          f"overlap {overlap / 1e9:.1f} s")
    report(analyze(e_frames, load_commands(emitter), w_frames, frame_ns))


if __name__ == "__main__":
    main()
