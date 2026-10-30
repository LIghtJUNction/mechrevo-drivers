#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Record and compare the two GM6PX0X fan-2 ECRR low-byte candidates.

No module loading, EC writes, fan mode changes, or artificial load is performed.
"""

import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import re
import time

SAMPLE_PATH = Path("/sys/module/gm6px0x_fan2_ecrr/sample")
DMI_ROOT = Path("/sys/class/dmi/id")
IDENTITY = {
    "sys_vendor": "MECHREVO",
    "product_name": "Kuangshi16Pro Series GM6PX0X",
    "board_name": "GM6PX0X",
    "bios_version": "N.1.09MRO07",
}
LINE = re.compile(
    r"attempt=([1-3]) order=(BD|DB) hi0=([0-9a-f]{2}) "
    r"b=([0-9a-f]{2}) d=([0-9a-f]{2}) "
    r"hi1=([0-9a-f]{2}) stable=([01])"
)


class SampleError(ValueError):
    pass


def check_identity(root=DMI_ROOT):
    for field, expected in IDENTITY.items():
        actual = (root / field).read_text(encoding="utf-8").strip()
        if actual != expected:
            raise SampleError(f"Unsupported {field}: expected {expected!r}, got {actual!r}")


def parse_sample(raw):
    """Keep all rollover attempts and accept only the last, stable one."""
    lines = raw.splitlines()
    if not 1 <= len(lines) <= 3:
        raise SampleError("Expected one to three complete attempt lines")
    attempts = []
    for index, line in enumerate(lines, start=1):
        match = LINE.fullmatch(line)
        if not match:
            raise SampleError("Malformed, non-byte, or unexpected ECRR attempt")
        attempt, order, hi0, b, d, hi1, stable = match.groups()
        if int(attempt) != index:
            raise SampleError("Missing, repeated, or out-of-order attempt")
        values = dict(hi0=int(hi0, 16), b=int(b, 16),
                      d=int(d, 16), hi1=int(hi1, 16))
        truly_stable = values["hi0"] == values["hi1"]
        if truly_stable != bool(int(stable)):
            raise SampleError("Firmware high-byte transition disagrees with stable marker")
        if truly_stable and index != len(lines):
            raise SampleError("Probe continued after a stable attempt")
        attempts.append({"attempt": index, "order": order,
                         **values, "stable": truly_stable})
    return attempts


def sample_row(number, raw, timestamp=None):
    attempts = parse_sample(raw)
    good = attempts[-1] if attempts[-1]["stable"] else None
    result = {
        "type": "sample",
        "number": number,
        "utc": timestamp or datetime.now(timezone.utc).isoformat(),
        "attempts": attempts,
        "rollovers": sum(not a["stable"] for a in attempts),
        "stable": good is not None,
        "count_046b": None,
        "count_046d": None,
    }
    if good:
        result["count_046b"] = (good["hi0"] << 8) | good["b"]
        result["count_046d"] = (good["hi0"] << 8) | good["d"]
    return result


def _correlation(xs, ys):
    mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
    covariance = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    vx = sum((x - mx) ** 2 for x in xs)
    vy = sum((y - my) ** 2 for y in ys)
    return covariance / math.sqrt(vx * vy) if vx and vy else None


def summarize(rows):
    stable = [r for r in rows if r.get("type") == "sample" and r["stable"]]
    b = [r["count_046b"] for r in stable]
    d = [r["count_046d"] for r in stable]
    total = sum(r["rollovers"] for r in rows if r.get("type") == "sample")
    errors = [r for r in rows if r.get("type") == "error"]
    exact = sum(x == y for x, y in zip(b, d))
    near = sum(abs(x - y) <= max(1, x * 0.01) for x, y in zip(b, d))
    corr = _correlation(b, d) if b else None
    b_span = max(b) - min(b) if b else 0
    d_span = max(d) - min(d) if d else 0
    sentinel_b = sum(x in (0, 0xffff) for x in b)
    sentinel_d = sum(x in (0, 0xffff) for x in d)
    b_low_ff = sum((x & 0xff) == 0xff for x in b)
    d_low_ff = sum((x & 0xff) == 0xff for x in d)

    # Diagnostic hints only. There is no independent tachometer reference.
    if len(stable) < 20 or b_span < 64 or len(set(b)) < 3:
        finding = "insufficient_observed_variation"
    elif (len(set(d)) >= 3 and near / len(stable) >= 0.95
          and corr is not None and corr >= 0.95
          and not sentinel_b and not sentinel_d):
        finding = "closely_tracking_in_observed_range"
    elif near / len(stable) < 0.5 or (len(set(d)) == 1 and len(set(b)) > 2):
        finding = "different_in_observed_range"
    else:
        finding = "mixed_or_inconclusive"

    return {
        "type": "summary",
        "captured_rows": sum(r.get("type") == "sample" for r in rows),
        "stable_rows": len(stable),
        "read_errors": len(errors),
        "discarded_high_rollovers": total,
        "exact_pair_matches": exact,
        "within_1pct_pair_matches": near,
        "low_b_distinct_values": len(set(x & 0xff for x in b)),
        "low_d_distinct_values": len(set(x & 0xff for x in d)),
        "b_count_range": [min(b), max(b)] if b else None,
        "d_count_range": [min(d), max(d)] if d else None,
        "d_count_span": d_span,
        "pearson_correlation": round(corr, 5) if corr is not None else None,
        "b_zero_or_ffff": sentinel_b,
        "d_zero_or_ffff": sentinel_d,
        "b_low_ff_rows": b_low_ff,
        "d_low_ff_rows": d_low_ff,
        "finding": finding,
        "independent_physical_rpm_validation": False,
        "driver_change_justified": False,
    }


def collect(count, interval, reader, write, sleep=time.sleep, clock=time.monotonic):
    if type(count) is not int or not 1 <= count <= 60:
        raise SampleError("samples must be 1..60")
    if (type(interval) not in (int, float) or not math.isfinite(interval)
            or not 0.5 <= interval <= 10):
        raise SampleError("interval must be finite, 0.5..10 seconds")
    rows = []
    for index in range(count):
        start = clock()
        try:
            row = sample_row(index + 1, reader())
        except (OSError, ValueError) as exc:
            row = {"type": "error", "number": index + 1,
                   "message": f"{type(exc).__name__}: {exc}"}
            write(row)
            rows.append(row)
            break
        row["elapsed_s"] = round(clock() - start, 6)
        write(row)
        rows.append(row)
        if index + 1 < count:
            sleep(max(0, interval - row["elapsed_s"]))
    summary = summarize(rows)
    summary["complete"] = len(rows) == count and not any(
        r["type"] == "error" for r in rows)
    write(summary)
    return summary


def analyze(path):
    with path.open(encoding="utf-8") as src:
        data = [json.loads(line) for line in src if line.strip()]
    if not data or data[0].get("type") != "header" or data[0].get("schema") != 1:
        raise SampleError("Not a fan-2 ECRR comparison capture")
    return summarize([row for row in data[1:] if row.get("type") in ("sample", "error")])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--live", action="store_true", help="Only on the exact qualified host")
    mode.add_argument("--analyze", type=Path, help="Read an existing JSONL capture offline")
    parser.add_argument("--output", type=Path, help="New JSONL capture path; never overwritten")
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--interval", type=float, default=1.0)
    args = parser.parse_args()

    try:
        if args.analyze:
            summary = analyze(args.analyze)
        else:
            if args.output is None:
                parser.error("--live requires --output")
            check_identity()
            if not SAMPLE_PATH.is_file():
                raise SampleError("Read-only diagnostic module is not loaded")
            fd = os.open(args.output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as out:
                def write(row):
                    out.write(json.dumps(row, sort_keys=True) + "\n")
                    out.flush()

                write({"type": "header", "schema": 1,
                       "host": IDENTITY, "ecrr": r"\_SB.INOU.ECRR",
                       "registers": ["046b", "046c", "046d"],
                       "read_only_probe": True})
                summary = collect(args.samples, args.interval,
                                  lambda: SAMPLE_PATH.read_text(encoding="ascii"),
                                  write)
        print(json.dumps(summary, indent=2, sort_keys=True))
        return 0 if summary.get("complete", True) else 1
    except (OSError, ValueError) as exc:
        parser.exit(1, str(exc) + "\n")


if __name__ == "__main__":
    raise SystemExit(main())
