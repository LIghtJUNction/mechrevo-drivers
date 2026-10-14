# Uniwill ioctl regression tests

Run from the repository root:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_uniwill_runtime.py -v
```

Requires Python 3 and a C compiler. The tests extract the actual ioctl and
profile/TDP functions and inject usercopy and EC failures. Fan internals and
the Clevo dispatcher are mocked. No module is installed and no hardware is
accessed. Passing these tests does not validate physical fans or power limits.

The 46 cases include all 16 combinations of the existing two-profile,
three-profile, LED-only and custom-TDP flags. Public profile writes must
match the advertised count; rejected values cannot access the EC. Custom-only
devices still apply the required internal OVERBOOST mode after valid TDP
parameter checks. The raw MODE ioctl keeps its existing low-byte ABI.

For baseline sensitivity, set `UNIWILL_IO_SOURCE_ROOT` to an unpacked prior
source tree. The same fixture then extracts that tree's actual functions;
it does not emulate the missing capability checks. Use `ulimit -c 0` when
running intentional baseline failures to avoid creating core dumps.
