# shellcheck shell=sh
# One terminal style for the repository's scripts.
# Source it once: `. scripts/ui.sh`.
# Two line kinds.
# A status line is `[tag] message`, with the tag carrying the colour and the message the terminal's own foreground.
# A body line carries no tag and no indent: subprocess output, ui_plain elaborations and ui_command commands.
# Colour is off for a pipe, NO_COLOR or TERM=dumb, so captured output stays plain.

ui_init() {
    if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != "dumb" ]; then
        UI_ESC=$(printf '\033')
        UI_RESET="${UI_ESC}[0m"
        UI_STEP="${UI_ESC}[1;36m"
        UI_OK="${UI_ESC}[32m"
        UI_FAIL="${UI_ESC}[31m"
        UI_WARN="${UI_ESC}[33m"
        UI_DIM="${UI_ESC}[2m"
        UI_CMD="${UI_ESC}[1m"
        return
    fi
    UI_RESET='' UI_STEP='' UI_OK='' UI_FAIL='' UI_WARN='' UI_DIM='' UI_CMD=''
}
ui_init

# The status shape: a coloured tag, then the message in the default foreground.
ui_tag() { printf '%s[%s]%s %s\n' "$1" "$2" "$UI_RESET" "$3"; }

# A body line: no tag, no indent, no colour.
ui_plain() { printf '%s\n' "$1"; }

# A command line for the demo session: bold, no colour.
ui_command() { printf '%s%s%s\n' "$UI_CMD" "$1" "$UI_RESET"; }

# A block header: ui_phase TAG MESSAGE.
ui_phase() { ui_tag "$UI_STEP" "$1" "$2"; }
# The closing phase of a successful run.
ui_success() { ui_tag "$UI_OK" "$1" "$2"; }
# The closing phase of a failed run.
ui_error() { ui_tag "$UI_FAIL" "$1" "$2"; }
# A result line: [ok], [fail] or [warn].
ui_ok() { ui_tag "$UI_OK" "ok" "$1"; }
ui_fail() { ui_tag "$UI_FAIL" "fail" "$1"; }
ui_warn() { ui_tag "$UI_WARN" "warn" "$1"; }
# A distinct note such as [mock], [exit] or [cast].
ui_note() { ui_tag "$UI_DIM" "$1" "$2"; }
