# shellcheck shell=bash
#
# Shared by scripts/dev-up, dev-down and dev-status. Sourced, never run.
# How the three are used is in guide/06-local-dev.md.
#
# Written for bash 3.2 (what macOS ships) and later, on macOS and Linux: no
# associative arrays, no `mapfile`, no `wait -n`, no `setsid`. One more thing
# 3.2 needs: a variable directly before a non-ASCII character is written
# ${name}, because in a UTF-8 locale it takes the character's first byte for
# part of the name ("$1” is…" fails with "unbound variable").
#
# Rules these scripts keep:
#   - They signal only a process whose pid dev-up recorded AND whose command
#     line still is what dev-up started. Anything else is named and left alone.
#   - They never open the data directory's files (leaf.conf holds the bot
#     token and the storage keys), and print no environment variable's value
#     except the settings listed in dev.env.example.
#   - Pid files and logs live in the runtime directory (.dev-run), which is
#     git-ignored and is never the data directory.

DEV_SCRIPTS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DEV_REPO=$(cd "$DEV_SCRIPTS/.." && pwd)

# The settings dev.env may hold. A name that is not here is reported (by name
# only) and ignored, so a typo does not pass silently.
DEV_ENV_KEYS="LEAF_DEV_PORT LEAF_DEV_DATA_DIR LEAF_DEV_TUNNEL_CONFIG
LEAF_DEV_TUNNEL_NAME LEAF_DEV_PUBLIC_URL LEAF_DEV_AVD LEAF_DEV_EMULATOR_ARGS
ANDROID_HOME LOG_LEVEL DEV_GUILD_ID LEAF_DEV_RUN_DIR LEAF_DEV_CLOUDFLARED
LEAF_DEV_WAIT_LEAF LEAF_DEV_WAIT_GATEWAY LEAF_DEV_WAIT_TUNNEL
LEAF_DEV_WAIT_EMULATOR LEAF_DEV_WAIT_STOP LEAF_DEV_WAIT_STOP_EMULATOR"

# The ones that are paths: a leading ~/ is expanded, and a relative path is
# taken from the repository root.
DEV_ENV_PATH_KEYS="LEAF_DEV_DATA_DIR LEAF_DEV_TUNNEL_CONFIG ANDROID_HOME LEAF_DEV_RUN_DIR"

# Seconds between two tries while waiting for something to answer.
DEV_POLL=0.2
# Lines of a log shown when a step fails.
DEV_TAIL_LINES=15

# Set by dev_failed; the exit status of dev-up and dev-down.
DEV_FAILED=0

# ---------------------------------------------------------------- output

# One line per step: the step's name is printed before the step runs (so a
# slow one shows what is being waited for), its result after.
dev_step() { printf '%-9s ' "$1"; }
dev_ok() { printf '%-8s %s\n' ok "$*"; }
dev_skip() { printf '%-8s %s\n' skip "$*"; }
dev_failed() {
  printf '%-8s %s\n' failed "$*"
  DEV_FAILED=1
}
# A whole line at once: name, where it stands, a sentence (dev-status).
dev_line() { printf '%-9s %-8s %s\n' "$1" "$2" "$3"; }
# A line under a step's line.
dev_more() { dev_line '' '' "$*"; }

dev_die() {
  printf '%s: %s\n' "${0##*/}" "$*" >&2
  exit 2
}

# The last lines of a log, under a failed step.
dev_log_tail() {
  local log=$1 line
  if [ ! -s "$log" ]; then
    dev_more "(nothing was written to $(dev_show_path "$log"))"
    return 0
  fi
  dev_more "last lines of $(dev_show_path "$log"):"
  tail -n "$DEV_TAIL_LINES" "$log" | while IFS= read -r line; do
    dev_more "| $line"
  done
}

# ---------------------------------------------------------------- settings

dev_is_one_of() {
  local wanted=$1 name
  shift
  # shellcheck disable=SC2048  # a word list, split on purpose
  for name in $*; do
    [ "$name" = "$wanted" ] && return 0
  done
  return 1
}

# Reads NAME=value lines from the settings file into shell variables. A value
# already in the environment wins, so `LEAF_DEV_PORT=3999 scripts/dev-up`
# overrides the file for one run; within the file the last line for a name
# wins. The file is read as data, never executed.
dev_read_env_file() {
  local file=$1 line key value quote rest after number=0 from_environment=
  [ -f "$file" ] || return 0
  for key in $DEV_ENV_KEYS; do
    [ -n "${!key+set}" ] && from_environment="$from_environment $key"
  done
  while IFS= read -r line || [ -n "$line" ]; do
    number=$((number + 1))
    line=${line#"${line%%[![:space:]]*}"}
    line=${line%"${line##*[![:space:]]}"}
    case $line in
    '' | '#'*) continue ;;
    esac
    line=${line#export }
    key=${line%%=*}
    value=${line#*=}
    case $line in
    *=*) ;;
    *) key= ;;
    esac
    case $key in
    '' | [0-9]* | *[!A-Z0-9_]*)
      # Only the line number: the line could be a pasted credential (and a
      # base64 one ends in `=`, so what is before it is no name to print).
      printf '%s: line %s is not NAME=value; ignored\n' "$file" "$number" >&2
      continue
      ;;
    esac
    if ! dev_is_one_of "$key" "$DEV_ENV_KEYS"; then
      # The name only, never the value, for the same reason.
      printf '%s: line %s sets %s, which these scripts do not read; ignored\n' \
        "$file" "$number" "$key" >&2
      continue
    fi
    case $value in
    \"*\"*) quote=\" ;;
    \'*\'*) quote=\' ;;
    *) quote= ;;
    esac
    if [ -n "$quote" ]; then
      # Quoted: the value is what is between the quotes, when nothing but a
      # comment follows the closing one. Anything else is kept as written.
      rest=${value#"$quote"}
      after=${rest#*"$quote"}
      after=${after#"${after%%[![:space:]]*}"}
      case $after in
      '' | '#'*) value=${rest%%"$quote"*} ;;
      esac
    else
      # Unquoted: a trailing " # comment" is not part of the value.
      value=${value%%[[:space:]]#*}
      value=${value%"${value##*[![:space:]]}"}
    fi
    dev_is_one_of "$key" "$from_environment" && continue
    printf -v "$key" '%s' "$value"
  done <"$file"
}

# A path setting as an absolute path: ~/ is the home directory, and a relative
# path starts at the repository root.
dev_abs_path() {
  local path=$1
  case $path in
  \~) path=$HOME ;;
  \~/*) path=$HOME/${path#\~/} ;;
  esac
  case $path in
  /*) ;;
  *) path=$DEV_REPO/$path ;;
  esac
  # No trailing slash, so two spellings of one directory compare equal.
  while [ "${path%/}" != "$path" ] && [ "$path" != / ]; do
    path=${path%/}
  done
  printf '%s\n' "$path"
}

# An absolute path as the file system has it, so two spellings of one
# directory come out the same: the part that exists is asked for its real
# path (symlinks followed, `.` and `..` gone, and on a file system that
# ignores case, the case it is stored in), and the part that does not exist
# yet is added as written.
dev_physical_path() {
  local path=$1 rest='' part real
  while [ ! -d "$path" ] && [ "$path" != / ]; do
    rest=${path##*/}/$rest
    path=${path%/*}
    [ -n "$path" ] || path=/
  done
  # The pwd program, not the shell's own: the shell's keeps the case that was
  # typed, the program asks the system.
  real=$(cd "$path" 2>/dev/null && { env pwd -P 2>/dev/null || pwd -P; }) || real=$path
  while [ -n "$rest" ]; do
    part=${rest%%/*}
    rest=${rest#*/}
    case $part in
    '' | .) ;;
    ..)
      real=${real%/*}
      [ -n "$real" ] || real=/
      ;;
    *) real=${real%/}/$part ;;
    esac
  done
  printf '%s\n' "$real"
}

# A path as a message shows it: from the repository root when it is inside
# the repository (where these scripts are run from), whole otherwise.
dev_show_path() {
  case $1 in
  "$DEV_REPO"/*) printf '%s\n' "${1#"$DEV_REPO"/}" ;;
  *) printf '%s\n' "$1" ;;
  esac
}

# A whole number of seconds the shell can count with: digits, six at most.
dev_is_seconds() {
  case $1 in
  '' | *[!0-9]* | ???????*) return 1 ;;
  esac
  return 0
}

# Loads the settings and derives everything the scripts use. $1 is the port
# from --port (empty for none). Dies with a sentence when a value is unusable.
dev_load_settings() {
  local port_flag=${1:-} port_given key run_real data_real
  # scripts/dev.env is optional. A file that was named is not: carrying on
  # without it would act on the default environment instead of the one meant.
  if [ -n "${LEAF_DEV_ENV_FILE:-}" ]; then
    DEV_ENV_FILE=$LEAF_DEV_ENV_FILE
    [ -f "$DEV_ENV_FILE" ] ||
      dev_die "there is no settings file at $DEV_ENV_FILE. Correct LEAF_DEV_ENV_FILE, or unset it to use scripts/dev.env."
  else
    DEV_ENV_FILE=$DEV_SCRIPTS/dev.env
  fi
  dev_read_env_file "$DEV_ENV_FILE"

  port_given=${port_flag:-${LEAF_DEV_PORT:-3777}}
  DEV_PORT=$port_given
  # Without leading zeros: 08080 is 8080, and leaf is given the number.
  while [ "${DEV_PORT#0}" != "$DEV_PORT" ]; do
    DEV_PORT=${DEV_PORT#0}
  done
  # At most five digits, so the comparison below is one the shell can make.
  case $DEV_PORT in
  '' | *[!0-9]* | ??????*) dev_die "the port must be a number from 1 to 65535, not “${port_given}”." ;;
  esac
  if [ "$DEV_PORT" -gt 65535 ]; then
    dev_die "the port must be a number from 1 to 65535, not “${port_given}”."
  fi

  DEV_DATA_DIR=$(dev_abs_path "${LEAF_DEV_DATA_DIR:-data}")
  DEV_RUN_DIR=$(dev_abs_path "${LEAF_DEV_RUN_DIR:-.dev-run}")
  # Logs and pid files never go where leaf.conf is. Compared as the file
  # system has them, so ./data, a symlink to the data directory, or DATA on a
  # file system that ignores case, is not a way around.
  run_real=$(dev_physical_path "$DEV_RUN_DIR")
  data_real=$(dev_physical_path "$DEV_DATA_DIR")
  case $run_real/ in
  "${data_real%/}"/*)
    dev_die "the runtime directory ($DEV_RUN_DIR) must not be the data directory or inside it. Change LEAF_DEV_RUN_DIR."
    ;;
  esac
  DEV_STATIC_DIR=$DEV_REPO/activity/dist

  DEV_TUNNEL_CONFIG=
  [ -n "${LEAF_DEV_TUNNEL_CONFIG:-}" ] && DEV_TUNNEL_CONFIG=$(dev_abs_path "$LEAF_DEV_TUNNEL_CONFIG")
  DEV_TUNNEL_NAME=${LEAF_DEV_TUNNEL_NAME:-}
  DEV_PUBLIC_URL=${LEAF_DEV_PUBLIC_URL:-}
  DEV_PUBLIC_URL=${DEV_PUBLIC_URL%/}
  DEV_CLOUDFLARED=${LEAF_DEV_CLOUDFLARED:-cloudflared}

  DEV_AVD=${LEAF_DEV_AVD:-}
  DEV_EMULATOR_ARGS=${LEAF_DEV_EMULATOR_ARGS:-}
  DEV_ANDROID_HOME=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
  [ -n "$DEV_ANDROID_HOME" ] && DEV_ANDROID_HOME=$(dev_abs_path "$DEV_ANDROID_HOME")

  DEV_WAIT_LEAF=${LEAF_DEV_WAIT_LEAF:-30}
  DEV_WAIT_GATEWAY=${LEAF_DEV_WAIT_GATEWAY:-20}
  DEV_WAIT_TUNNEL=${LEAF_DEV_WAIT_TUNNEL:-60}
  DEV_WAIT_EMULATOR=${LEAF_DEV_WAIT_EMULATOR:-300}
  DEV_WAIT_STOP=${LEAF_DEV_WAIT_STOP:-15}
  # The emulator writes its snapshot when it is asked to stop, and a kill in
  # the middle of that can leave a snapshot that does not load.
  DEV_WAIT_STOP_EMULATOR=${LEAF_DEV_WAIT_STOP_EMULATOR:-120}
  for key in DEV_WAIT_LEAF DEV_WAIT_GATEWAY DEV_WAIT_TUNNEL DEV_WAIT_EMULATOR DEV_WAIT_STOP \
    DEV_WAIT_STOP_EMULATOR; do
    dev_is_seconds "${!key}" ||
      dev_die "LEAF_$key must be a whole number of seconds (at most 999999), not “${!key}”."
    # As a decimal number: with a leading zero, 08 would be read as octal.
    printf -v "$key" '%s' "$((10#${!key}))"
  done

  DEV_LOCAL_URL=http://127.0.0.1:$DEV_PORT
}

dev_require() {
  command -v "$1" >/dev/null 2>&1 || dev_die "$1 is needed and was not found on PATH."
}

# ---------------------------------------------------------------- processes

# The full command line of a process (nothing when there is none).
dev_proc_command() { ps -ww -o command= -p "$1" 2>/dev/null; }

# The name of a process's program, without its arguments: what a message
# names a process by, since arguments can hold anything.
dev_proc_name() {
  local comm
  comm=$(ps -o comm= -p "$1" 2>/dev/null) || return 1
  comm=${comm%"${comm##*[![:space:]]}"}
  [ -n "$comm" ] || return 1
  printf '%s\n' "${comm##*/}"
}

# Whether a process exists and has not exited (a zombie has).
dev_proc_alive() {
  local stat
  kill -0 "$1" 2>/dev/null || return 1
  stat=$(ps -o stat= -p "$1" 2>/dev/null) || return 1
  case $stat in
  '' | Z*) return 1 ;;
  esac
  return 0
}

# The pids of running processes whose command line contains every argument.
# A read-only look: nothing found here is ever signalled.
dev_find_process() {
  local pid cmd needle hit
  ps -Aww -o pid=,command= 2>/dev/null | while read -r pid cmd; do
    [ "$pid" = "$$" ] && continue
    hit=1
    for needle in "$@"; do
      case "$cmd " in
      *"$needle"*) ;;
      *)
        hit=0
        break
        ;;
      esac
    done
    [ "$hit" -eq 1 ] && printf '%s\n' "$pid"
  done
  return 0
}

# ---------------------------------------------------------------- what dev-up started
#
# For each thing dev-up starts (leaf, tunnel, emulator) the runtime directory
# holds NAME.pid (the pid), NAME.info (key=value lines: `sig`, a piece of the
# command line that process must still have, and what the other scripts need
# to know about it) and NAME.log.

dev_pid_file() { printf '%s/%s.pid\n' "$DEV_RUN_DIR" "$1"; }
dev_info_file() { printf '%s/%s.info\n' "$DEV_RUN_DIR" "$1"; }
dev_log_file() { printf '%s/%s.log\n' "$DEV_RUN_DIR" "$1"; }

dev_info_get() {
  local file line
  file=$(dev_info_file "$1")
  [ -f "$file" ] || return 0
  while IFS= read -r line; do
    case $line in
    "$2="*)
      printf '%s\n' "${line#*=}"
      return 0
      ;;
    esac
  done <"$file"
}

dev_info_add() { printf '%s=%s\n' "$2" "$3" >>"$(dev_info_file "$1")"; }

dev_forget() { rm -f "$(dev_pid_file "$1")" "$(dev_info_file "$1")"; }

# Creates the runtime directory when it is not there, readable by this
# account only: leaf's log holds the setup code.
dev_make_run_dir() {
  [ -d "$DEV_RUN_DIR" ] && return 0
  if ! mkdir -p "$DEV_RUN_DIR" || ! chmod 700 "$DEV_RUN_DIR"; then
    dev_die "cannot create the runtime directory $DEV_RUN_DIR."
  fi
}

# Where a recorded process stands. Sets DEV_PID (empty when nothing usable is
# recorded) and returns:
#   0  running, and its command line is still what dev-up started
#   1  nothing recorded
#   2  recorded, but that process has exited
#   3  recorded, but the pid now belongs to some other process
dev_component_state() {
  local name=$1 file pid sig cmd
  DEV_PID=
  file=$(dev_pid_file "$name")
  [ -f "$file" ] || return 1
  pid=$(head -n 1 "$file" 2>/dev/null)
  case $pid in
  '' | *[!0-9]*) return 2 ;;
  esac
  DEV_PID=$pid
  dev_proc_alive "$pid" || return 2
  sig=$(dev_info_get "$name" sig)
  [ -n "$sig" ] || return 3
  cmd=$(dev_proc_command "$pid")
  case $cmd in
  *"$sig"*) return 0 ;;
  esac
  return 3
}

# Starts a command in the background, detached from this terminal, with its
# output in NAME.log, and records it. Sets DEV_PID.
#   dev_spawn NAME SIGNATURE COMMAND [ARG...]
dev_spawn() {
  local name=$1 sig=$2 log
  shift 2
  log=$(dev_log_file "$name")
  # Keep the previous run's log once: the one a failure is usually in.
  [ -f "$log" ] && mv -f "$log" "$log.1"
  : >"$log" || dev_die "cannot write $log."
  dev_forget "$name"
  # The signature first and the pid straight after the start, so an interrupt
  # at any point leaves either nothing recorded or a process dev-down can stop.
  dev_info_add "$name" sig "$sig"
  # Job control gives the child a process group of its own, so Ctrl-C in this
  # terminal (now or later) does not reach it; nohup covers the terminal
  # closing. The shell and the child both make that group; on macOS the
  # slower of the two now and then gets "child setpgid: Operation not
  # permitted" for a group that is already made, and bash 3.2 prints it. The
  # braces send that one line nowhere; everything the command itself prints
  # goes to the log.
  set -m
  { nohup "$@" >"$log" 2>&1 </dev/null & } 2>/dev/null
  DEV_PID=$!
  printf '%s\n' "$DEV_PID" >"$(dev_pid_file "$name")"
  set +m
  # Not a job of this shell any more: no "Terminated" line if it is stopped.
  disown "$DEV_PID" 2>/dev/null
}

# Stops what is recorded under a name: SIGTERM, and SIGKILL if it is still
# there after the wait (DEV_WAIT_STOP seconds unless another is given).
#   dev_stop_recorded NAME [SECONDS]
# Both signals are sent only after the check made everywhere else: the pid is
# the one dev-up recorded and its command line is still what dev-up started.
# Sets DEV_PID and DEV_STOP_HOW:
#   term     it stopped when asked
#   kill     it was still there after the wait and was killed
#   changed  while waiting, the pid became some other process: left alone
# Returns 1 when there was nothing of dev-up's to stop, 2 when it survived
# SIGKILL.
dev_stop_recorded() {
  local name=$1 wait=${2:-$DEV_WAIT_STOP} pid deadline tries=0 state
  DEV_STOP_HOW=
  dev_component_state "$name" || return 1
  pid=$DEV_PID
  DEV_STOP_HOW=term
  kill -TERM "$pid" 2>/dev/null
  deadline=$((SECONDS + wait))
  while dev_proc_alive "$pid"; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      # The wait can be minutes: look again at whose process this is.
      dev_component_state "$name"
      state=$?
      if [ "$state" -ne 0 ] || [ "$DEV_PID" != "$pid" ]; then
        DEV_PID=$pid
        dev_proc_alive "$pid" && DEV_STOP_HOW=changed
        return 0
      fi
      DEV_STOP_HOW=kill
      kill -KILL "$pid" 2>/dev/null
      while dev_proc_alive "$pid"; do
        tries=$((tries + 1))
        [ "$tries" -gt 50 ] && return 2
        sleep "$DEV_POLL"
      done
      return 0
    fi
    sleep "$DEV_POLL"
  done
  return 0
}

# ---------------------------------------------------------------- ports and HTTP

# The pids of the processes listening on a TCP port, one a line; nothing when
# there is none this account can see.
dev_port_holders() {
  local port=$1 pids=
  if command -v lsof >/dev/null 2>&1; then
    pids=$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fp 2>/dev/null | sed -n 's/^p//p')
  fi
  if [ -z "$pids" ] && command -v ss >/dev/null 2>&1; then
    pids=$(ss -ltnpH "sport = :$port" 2>/dev/null |
      sed -n 's/.*pid=\([0-9][0-9]*\).*/\1/p')
  fi
  [ -n "$pids" ] && printf '%s\n' "$pids"
  return 0
}

# The first of them.
dev_port_holder() { dev_port_holders "$1" | head -n 1; }

# Whether the process with this pid is the one listening on the port. Also
# true when no listener can be seen at all: there is nothing to compare with.
dev_port_is_held_by() {
  local holders holder
  holders=$(dev_port_holders "$1")
  [ -n "$holders" ] || return 0
  for holder in $holders; do
    [ "$holder" = "$2" ] && return 0
  done
  return 1
}

# curl, as every request here makes it: quiet, and never through a proxy the
# environment names when the address is on this machine (a proxy cannot reach
# 127.0.0.1 here, and the answer would be the proxy's, not leaf's).
dev_curl() { curl -s --noproxy 127.0.0.1,localhost "$@" 2>/dev/null; }

# Whether anything accepts connections on 127.0.0.1:PORT (curl's status 7 is
# "connection refused").
dev_port_answers() {
  local rc=0
  dev_curl -o /dev/null --max-time 2 "http://127.0.0.1:$1/" || rc=$?
  [ "$rc" -ne 7 ]
}

# A sentence naming whatever holds the port, or nothing when it is free.
dev_port_conflict() {
  local port=$1 pid name
  pid=$(dev_port_holder "$port")
  if [ -n "$pid" ]; then
    name=$(dev_proc_name "$pid") || name="a process"
    printf 'port %s is held by %s (pid %s)\n' "$port" "$name" "$pid"
  elif dev_port_answers "$port"; then
    printf 'port %s is held by a process this account cannot see (sudo lsof -nP -iTCP:%s -sTCP:LISTEN names it)\n' \
      "$port" "$port"
  fi
}

# The HTTP status of a GET, `000` when nothing answered. Redirects are not
# followed: the status itself tells setup mode from run mode.
dev_http_code() {
  local code
  code=$(dev_curl -o /dev/null -w '%{http_code}' --max-time 3 "$1")
  printf '%s\n' "${code:-000}"
}

dev_http_body() { dev_curl --max-time 3 "$1"; }

# The value of a string field in a one-line JSON object, unescaped.
dev_json_string() {
  printf '%s\n' "$1" |
    LC_ALL=C sed -nE 's/.*"'"$2"'"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*/\1/p' |
    LC_ALL=C sed -e 's/\\"/"/g' -e 's/\\\\/\\/g'
}

# ---------------------------------------------------------------- leaf

# Which mode a leaf at DEV_LOCAL_URL is in: prints `run` (it answers
# /healthz), `setup` (it serves the setup page), or nothing.
dev_leaf_mode() {
  if [ "$(dev_http_code "$DEV_LOCAL_URL/healthz")" = 200 ]; then
    echo run
  elif [ "$(dev_http_code "$DEV_LOCAL_URL/setup")" = 200 ]; then
    echo setup
  fi
}

# The path that answers 200 in a mode: what a check of the public URL asks.
dev_mode_path() {
  if [ "$1" = setup ]; then echo /setup; else echo /healthz; fi
}

# The one-time setup code leaf logged when it started in setup mode.
dev_setup_code() {
  # As bytes: a log is not promised to be valid UTF-8, and sed stops at a
  # byte sequence its locale cannot read.
  LC_ALL=C sed -n 's/.*setup code: *\([A-Za-z0-9-]*\).*/\1/p' "$(dev_log_file leaf)" 2>/dev/null |
    tail -n 1
}

# The line under a setup-mode leaf: the code the setup page asks for.
dev_setup_code_line() {
  local code
  code=$(dev_setup_code)
  if [ -n "$code" ]; then
    dev_more "setup code: $code"
  else
    dev_more "no setup code in $(dev_show_path "$(dev_log_file leaf)"). leaf logs it at info level: unset LOG_LEVEL, then run scripts/dev-down and scripts/dev-up."
  fi
}

# Reads GET /api/status. Sets DEV_GATEWAY (`starting`, `online`, `error`, or
# empty when there was no usable answer), DEV_GATEWAY_DETAIL and
# DEV_GATEWAY_NOTICE.
dev_read_gateway() {
  local body
  body=$(dev_http_body "$DEV_LOCAL_URL/api/status")
  DEV_GATEWAY=$(dev_json_string "$body" gateway)
  DEV_GATEWAY_DETAIL=$(dev_json_string "$body" detail)
  DEV_GATEWAY_NOTICE=$(dev_json_string "$body" notice)
}

# ---------------------------------------------------------------- emulator

# Where the SDK's emulator and adb are. Sets DEV_EMULATOR_BIN and DEV_ADB_BIN
# (empty when not found).
dev_find_sdk_tools() {
  DEV_EMULATOR_BIN=
  DEV_ADB_BIN=
  if [ -n "$DEV_ANDROID_HOME" ]; then
    [ -x "$DEV_ANDROID_HOME/emulator/emulator" ] && DEV_EMULATOR_BIN=$DEV_ANDROID_HOME/emulator/emulator
    [ -x "$DEV_ANDROID_HOME/platform-tools/adb" ] && DEV_ADB_BIN=$DEV_ANDROID_HOME/platform-tools/adb
  fi
  [ -n "$DEV_EMULATOR_BIN" ] || DEV_EMULATOR_BIN=$(command -v emulator 2>/dev/null)
  [ -n "$DEV_ADB_BIN" ] || DEV_ADB_BIN=$(command -v adb 2>/dev/null)
  return 0
}

# The adb serial (emulator-5554, …) of the running emulator whose AVD is
# DEV_AVD; nothing while adb does not list it yet.
dev_emulator_serial() {
  local serial name
  "$DEV_ADB_BIN" devices 2>/dev/null | sed -n 's/^\(emulator-[0-9][0-9]*\)[[:space:]].*/\1/p' |
    while read -r serial; do
      name=$("$DEV_ADB_BIN" -s "$serial" emu avd name 2>/dev/null | head -n 1 | tr -d '\r')
      if [ "$name" = "$DEV_AVD" ]; then
        printf '%s\n' "$serial"
        break
      fi
    done
}

# Whether Android on that serial has finished booting.
dev_emulator_booted() {
  local done_flag
  done_flag=$("$DEV_ADB_BIN" -s "$1" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r\n ')
  [ "$done_flag" = 1 ]
}

# The pid of an emulator running DEV_AVD that dev-up did not start.
dev_foreign_emulator() { dev_find_process "-avd $DEV_AVD " | head -n 1; }

# ---------------------------------------------------------------- one at a time

# dev-up and dev-down change the same pid files, so only one runs at a time.
dev_lock() {
  local lock=$DEV_RUN_DIR/lock holder
  if ! mkdir "$lock" 2>/dev/null; then
    holder=$(head -n 1 "$lock/pid" 2>/dev/null)
    case $holder in
    '' | *[!0-9]*) holder= ;;
    esac
    if [ -n "$holder" ] && dev_proc_alive "$holder"; then
      printf '%s: another dev-up or dev-down is running (pid %s). Wait for it, or if it is not one of them, remove %s.\n' \
        "${0##*/}" "$holder" "$lock" >&2
      exit 1
    fi
    # Left behind by a run that was killed.
    rm -f "$lock/pid"
    rmdir "$lock" 2>/dev/null
    mkdir "$lock" 2>/dev/null || dev_die "cannot create $lock."
  fi
  printf '%s\n' "$$" >"$lock/pid"
  DEV_LOCK=$lock
}

dev_unlock() {
  [ -n "${DEV_LOCK:-}" ] || return 0
  rm -f "$DEV_LOCK/pid"
  rmdir "$DEV_LOCK" 2>/dev/null
  DEV_LOCK=
}
