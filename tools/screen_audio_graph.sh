#!/bin/sh
# Run a command inside a private PipeWire + WirePlumber graph (own runtime dir and D-Bus):
# the desktop's real audio is never touched, nothing is played to a device.
# Example: tools/screen_audio_graph.sh cargo test --lib computer::audio -- --ignored
set -eu
root=$(mktemp -d /tmp/solaris-pw.XXXXXX)
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$root"' EXIT
mkdir -p "$root/config" "$root/state" "$root/data" "$root/home"
export SOLARIS_PW_ROOT="$root"
exec dbus-run-session -- sh -c '
  export XDG_RUNTIME_DIR="$SOLARIS_PW_ROOT" PIPEWIRE_RUNTIME_DIR="$SOLARIS_PW_ROOT" \
    XDG_CONFIG_HOME="$SOLARIS_PW_ROOT/config" XDG_STATE_HOME="$SOLARIS_PW_ROOT/state" \
    XDG_DATA_HOME="$SOLARIS_PW_ROOT/data" HOME="$SOLARIS_PW_ROOT/home"
  unset DISPLAY WAYLAND_DISPLAY
  pipewire >"$SOLARIS_PW_ROOT/pipewire.log" 2>&1 &
  wireplumber >"$SOLARIS_PW_ROOT/wireplumber.log" 2>&1 &
  # WirePlumber publishes its Midi-Bridge node once it manages the graph: wait for that event.
  timeout 30 pw-mon 2>/dev/null | grep -m1 "Midi-Bridge" >/dev/null
  "$@"
  status=$?
  kill $(jobs -p) 2>/dev/null
  exit $status
'
