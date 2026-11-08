#!/usr/bin/env bash
# End-to-end voice latency between two clients and a real Mumble server.
# The server runs in a container; both clients are native processes, so they
# share this machine's clock.
#
#   dev/e2e/run.sh probe    # timestamp in every packet: the client -> server -> client hop
#   dev/e2e/run.sh speech   # speech through the real mic and playback pipelines (macOS: uses `say`)
#
# The receiver's exit code is the result.
set -euo pipefail
cd "$(dirname "$0")/../.."

mode=${1:?usage: run.sh probe|speech}
compose=(podman compose -f dev/e2e/compose.yaml)
out=target/e2e
mkdir -p "$out"

# Not the workspace's fat-LTO release profile: slow to build, no benefit here.
CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  nice cargo build --release --locked -p e2e-latency
bin=target/release/e2e-latency

case $mode in
  probe) receiver=receiver sender=sender ;;
  speech)
    receiver=speech-receiver sender=speech-sender
    say -o "$out/speech.aiff" "Testing, one two three. The quick brown fox jumps over the lazy dog."
    afconvert -f WAVE -d LEI16@22050 -c 1 "$out/speech.aiff" "$out/speech.wav"
    export SPEECH_WAV="$out/speech.wav"
    ;;
  *) echo "unknown mode: $mode" >&2; exit 2 ;;
esac

sender_pid=
cleanup() {
  [[ -n $sender_pid ]] && kill "$sender_pid" 2>/dev/null || true
  "${compose[@]}" down
}
trap cleanup EXIT

"${compose[@]}" up -d
# The port opens before the server is ready to talk; the clients retry.
until nc -z localhost 64738; do sleep 1; done

export PROBE_TIMEOUT_SECS=60
"$bin" "$receiver" &
receiver_pid=$!
"$bin" "$sender" &
sender_pid=$!

wait "$receiver_pid"
