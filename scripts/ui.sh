# shellcheck shell=sh
# One terminal style for the repository's scripts.
# Source it once: `. scripts/ui.sh`.
# Every line is `[tag] message`: the tag carries the colour, the message keeps the terminal's own foreground.
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

# The one line shape: a coloured tag, then the message in the default foreground.
ui_tag() { printf '%s[%s]%s %s\n' "$1" "$2" "$UI_RESET" "$3"; }

# A command line for the demo session: bold, no colour.
ui_command() { printf '%s%s%s\n' "$UI_CMD" "$1" "$UI_RESET"; }

# A phase: ui_step TAG MESSAGE.
ui_step() { ui_tag "$UI_STEP" "$1" "$2"; }
# The closing phase of a successful run.
ui_success() { ui_tag "$UI_OK" "$1" "$2"; }
# The closing phase of a failed run.
ui_error() { ui_tag "$UI_FAIL" "$1" "$2"; }
# A result line: [ok] or [fail].
ui_ok() { ui_tag "$UI_OK" "ok" "$1"; }
ui_fail() { ui_tag "$UI_FAIL" "fail" "$1"; }
ui_warn() { ui_tag "$UI_WARN" "warn" "$1"; }
# Secondary text: ui_note TAG MESSAGE.
ui_note() { ui_tag "$UI_DIM" "$1" "$2"; }
ui_hint() { ui_tag "$UI_DIM" "hint" "$1"; }
