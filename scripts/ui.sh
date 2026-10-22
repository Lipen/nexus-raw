# shellcheck shell=sh
# One terminal style for the repository's scripts.
# Source it once: `. scripts/ui.sh`.
# Status lines are `[tag] message`.
# Body lines carry no tag and no indent.
# Status, results and body go to stdout.
# Failures, warnings and hints go to stderr.
# Colour is decided per stream: [ -t 1 ] for stdout, [ -t 2 ] for stderr.
# NO_COLOR and TERM=dumb disable both.

ui_color_ok() {
    [ -t "$1" ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != "dumb" ]
}

ui_init() {
    UI_RESET='' UI_STEP='' UI_OK='' UI_DIM='' UI_CMD=''
    UI_ERR_RESET='' UI_ERR_FAIL='' UI_ERR_WARN='' UI_ERR_DIM=''
    if ui_color_ok 1; then
        UI_ESC=$(printf '\033')
        UI_RESET="${UI_ESC}[0m"
        UI_STEP="${UI_ESC}[1;36m"
        UI_OK="${UI_ESC}[32m"
        UI_DIM="${UI_ESC}[2m"
        UI_CMD="${UI_ESC}[1m"
    fi
    if ui_color_ok 2; then
        UI_ERR_ESC=$(printf '\033')
        UI_ERR_RESET="${UI_ERR_ESC}[0m"
        UI_ERR_FAIL="${UI_ERR_ESC}[31m"
        UI_ERR_WARN="${UI_ERR_ESC}[33m"
        UI_ERR_DIM="${UI_ERR_ESC}[2m"
    fi
}
ui_init

# The status shape: a coloured tag, then the message in the default foreground.
ui_tag() { printf '%s[%s]%s %s\n' "$1" "$2" "$UI_RESET" "$3"; }
# The stderr shape: the same, with the stderr palette.
ui_tag_err() { printf '%s[%s]%s %s\n' "$1" "$2" "$UI_ERR_RESET" "$3" >&2; }

# A body line: no tag, no indent, no colour.
ui_plain() { printf '%s\n' "$1"; }

# A command line for the demo session: bold, no colour.
ui_command() { printf '%s%s%s\n' "$UI_CMD" "$1" "$UI_RESET"; }

# A block header: ui_phase TAG MESSAGE.
ui_phase() { ui_tag "$UI_STEP" "$1" "$2"; }
# The closing phase of a successful run.
ui_success() { ui_tag "$UI_OK" "$1" "$2"; }
# The closing phase of a failed run.
ui_error() { ui_tag_err "$UI_ERR_FAIL" "$1" "$2"; }
# A result line: [ok], [fail] or [warn].
ui_ok() { ui_tag "$UI_OK" "ok" "$1"; }
ui_fail() { ui_tag_err "$UI_ERR_FAIL" "fail" "$1"; }
ui_warn() { ui_tag_err "$UI_ERR_WARN" "warn" "$1"; }
# A distinct note such as [mock], [exit] or [cast].
ui_note() { ui_tag "$UI_DIM" "$1" "$2"; }
# A fix or next step, with commands in backticks.
ui_hint() { ui_tag_err "$UI_ERR_DIM" "hint" "$1"; }
