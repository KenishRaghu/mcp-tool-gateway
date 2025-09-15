#!/bin/bash
# Runs inside the task container after the agent finishes (verifier mode: shared).
mkdir -p /logs/verifier
cp /app/audit.jsonl /logs/verifier/audit.jsonl 2>/dev/null || true
if python3 /tests/check.py /app/workspace /app/audit.jsonl | tee /logs/verifier/result.json; then
  echo 1 > /logs/verifier/reward.txt
else
  echo 0 > /logs/verifier/reward.txt
fi
