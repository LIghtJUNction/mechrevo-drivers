# WMI EC regression tests

Run from the repository root:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_uniwill_wmi_runtime.py -v
```

Requires Python 3 and a C compiler with AddressSanitizer and UndefinedBehaviorSanitizer.
The tests extract the actual driver functions and size constants and run them
with mocked allocator, ACPI and EC operations. They do not install modules or
access hardware. `UNIWILL_WMI_TEST_SOURCE` selects a previous source file for
regression comparisons; `UNIWILL_WMI_TEST_TMPDIR` selects the temporary directory.
