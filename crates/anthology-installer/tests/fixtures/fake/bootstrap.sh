#!/bin/sh
# A fake hooks/bootstrap.sh: what the guardrails plugin's real one prints with --quiet.
echo "bootstrap.sh $*" >>"$FAKE_STATE/calls.log"
if [ "$FAKE_BOOTSTRAP_FAIL" = 1 ]; then
    echo "status=failed:offline"
    exit 1
fi
bin=$CLAUDE_CONFIG_DIR/anthology/bin/0.0.0
if [ -f "$bin/anthology" ]; then
    echo "status=present"
else
    mkdir -p "$bin"
    : >"$bin/anthology"
    echo "status=installed"
fi
