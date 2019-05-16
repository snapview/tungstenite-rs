#!/usr/bin/env bash
# Author michael <themichaeleden@gmail.com>
set -euo pipefail
set -x
SOURCE_DIR=$(readlink -f "${BASH_SOURCE[0]}")
SOURCE_DIR=$(dirname "$SOURCE_DIR")
cd "${SOURCE_DIR}/.."
WSSERVER_PID=

function cleanup() {
    kill -9 ${WSSERVER_PID}
}
trap cleanup TERM EXIT

function test_diff() {
    DIFF=$(diff \
        <(jq -S 'del(."rust-websocket" | .. | .duration?)' 'autobahn/client-results.json') \
        <(jq -S 'del(."rust-websocket" | .. | .duration?)' 'autobahn/server/index.json') )

    if [[ $DIFF ]]; then
        echo Difference in results, either this is a regression or \
             one should update autobahn/server-results.json with the new results. \
             The results are:
        echo $DIFF
        exit 64
    fi
}

cargo build --release --example autobahn-server
./target/debug/examples/autobahn-server & WSSERVER_PID=$!
echo "Server PID: ${WSSERVER_PID}"
sleep 3
wstest -m fuzzingclient -s 'autobahn/fuzzingclient.json'
test_diff
