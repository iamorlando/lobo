#!/bin/bash
# Four real terminal panes, one native feed/replay clock/simulation.
set -euo pipefail
session=${LOBO_TMUX_SESSION:-lobo}
binary=${LOBO_BIN:-lobo}
command -v tmux >/dev/null
binary=$(command -v "$binary")
binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"
if tmux has-session -t "=$session" 2>/dev/null; then
  echo "tmux session $session already exists; use tmux attach -t $session" >&2
  exit 1
fi
owned=false
if [[ -n ${LOBO_SOCKET:-} ]]; then
  socket=$LOBO_SOCKET
else
  directory=$(mktemp -d /tmp/lobo-panes.XXXXXX)
  socket=$directory/feed.sock
  owned=true
  "$binary" session --socket "$socket" "$@" >"$directory/session.log" 2>&1 &
  feed_pid=$!
  for ((i=0; i<500; i++)); do
    [[ -S $socket ]] && break
    if ! kill -0 "$feed_pid" 2>/dev/null; then
      cat "$directory/session.log" >&2
      rm -rf "$directory"
      exit 1
    fi
    sleep 0.01
  done
  if [[ ! -S $socket ]]; then
    kill "$feed_pid" 2>/dev/null || true
    echo "Lobo session did not start; inspect $directory/session.log" >&2
    exit 1
  fi
fi
cleanup_failure() {
  if $owned; then
    "$binary" ctl --attach "$socket" --execute stop >/dev/null 2>&1 || true
  fi
}
trap cleanup_failure ERR
printf -v base '%q ' "$binary" --attach "$socket" --renderer "${LOBO_RENDERER:-auto}" --theme "${LOBO_THEME:-Acid Lime}"
left=$(tmux new-session -d -P -F '#{pane_id}' -s "$session" -x "${LOBO_COLUMNS:-180}" -y "${LOBO_ROWS:-52}" -e COLORTERM=truecolor "${base}candles")
right=$(tmux split-window -h -d -P -F '#{pane_id}' -t "$left" "${base}book")
tmux split-window -v -d -t "$left" "${base}flow"
tmux split-window -v -d -t "$right" "${base}simulate"
trap - ERR
if $owned; then
  # Detaching keeps the feed alive. Closing the last pane ends the owned session.
  (
    while tmux has-session -t "=$session" 2>/dev/null; do sleep 1; done
    "$binary" ctl --attach "$socket" --execute stop >/dev/null 2>&1 || true
    rm -rf "$directory"
  ) </dev/null >/dev/null 2>&1 &
fi
if [[ ${LOBO_DETACHED:-0} == 1 ]]; then
  echo "tmux attach -t $session"
else
  tmux attach-session -t "=$session"
fi
