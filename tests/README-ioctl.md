# Uniwill ioctl regression tests

Run from the repository root:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_uniwill_runtime.py -v
```

Requires Python 3 and a C compiler. The tests extract the actual ioctl and
profile/TDP functions and inject usercopy and EC failures. Fan internals and
the Clevo dispatcher are mocked. No module is installed and no hardware is
accessed. Passing these tests does not validate physical fans or power limits.
