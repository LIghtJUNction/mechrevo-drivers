"""Keep pre-curve regressions scoped to their original no-lease scenarios.

The separate fan-curve transaction suite runs the actual provider and engine.
These adapters supply only the newly added, absent-owner hooks, leaving every
extracted legacy driver function and its previous assertions unchanged.
"""


def without_curve_provider(source):
    # The opener may also begin the provider's multiline documentation comment.
    begin = "/* UW_CURVE_PROVIDER_BEGIN"
    end = "/* UW_CURVE_PROVIDER_END */"
    starts, ends = source.count(begin), source.count(end)
    if starts == ends == 0:
        return source
    if starts != 1 or ends != 1:
        raise AssertionError("Expected exactly one complete curve provider block")
    first, last = source.index(begin), source.index(end)
    if last <= first:
        raise AssertionError("Curve provider markers are out of order")
    return source[:first] + "/* No curve owner in these legacy regressions. */" + source[last + len(end):]


def no_curve_owner_hooks(source):
    hooks = (
        ("uw_curve_blocks_legacy_locked", "bool", "return false;"),
        ("uw_curve_revoke_locked", "int", "return 0;"),
        ("uw_curve_new_lifecycle_locked", "void", ""),
        ("uw_curve_note_legacy_write_locked", "void", ""),
    )
    return "\n".join(
        "static __attribute__((unused)) " + result + " " + name
        + "(void) { " + body + " }"
        for name, result, body in hooks if name + "(" in source
    )


def curve_ioctl_compat(source):
    """The old ioctl scenarios do not issue the new, separately tested ABI."""
    if "uw_fan_curve_ioctl(" not in source:
        return ""
    return '''
#include "uniwill_fan_curve_ioctl.h"
/* New curve commands belong to the separate real-provider ABI harness. */
static __attribute__((unused)) long uw_fan_curve_ioctl(struct file *file,
        unsigned int command, unsigned long argument)
{
    (void)file; (void)command; (void)argument;
    return -ENOTTY;
}
'''
