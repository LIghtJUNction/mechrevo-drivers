# Mini LED initialization regression tests

Run from the repository root:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_uniwill_mini_led_init.py
```

Requires Python 3 and a C compiler. The tests compile the actual initializer
with small sysfs and firmware-call mocks. They cover unsupported hardware,
registration failure and successful initialization without touching a panel.
Set `UNIWILL_KEYBOARD_HEADER` to compare a previous source file.
