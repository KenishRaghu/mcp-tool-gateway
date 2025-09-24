#!/usr/bin/env bash
# Sentinel gate for the gateway rollout.
#
#   infra/scripts/gate.sh environments/production.tfvars          # plan + policy check
#   infra/scripts/gate.sh environments/production.tfvars --apply  # ...then apply if allowed
#
# MOCK_AWS=1 plans without AWS credentials (used in CI; nothing is created).
set -euo pipefail

VARS="${1:?usage: gate.sh <tfvars> [--apply]}"
APPLY="${2:-}"
INFRA="$(cd "$(dirname "$0")/.." && pwd)"
TF="$INFRA/terraform"
SENT="$INFRA/sentinel"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK" "$TF/ci_override.tf" "$SENT/gate.hcl" "$SENT/mocks/current-plan.sentinel"' EXIT

if [[ "${MOCK_AWS:-}" == "1" ]]; then
  [[ "$APPLY" == "--apply" ]] && { echo "refusing to apply with MOCK_AWS=1" >&2; exit 2; }
  export AWS_ACCESS_KEY_ID=mock AWS_SECRET_ACCESS_KEY=mock
  cat > "$TF/ci_override.tf" <<'HCL'
provider "aws" {
  skip_credentials_validation = true
  skip_requesting_account_id  = true
  skip_metadata_api_check     = true
}
HCL
fi

echo "==> terraform plan ($VARS)"
terraform -chdir="$TF" init -input=false -backend=false >/dev/null
terraform -chdir="$TF" plan -input=false -var-file="$VARS" -out="$WORK/plan.tfplan" >/dev/null
terraform -chdir="$TF" show -json "$WORK/plan.tfplan" | python3 "$INFRA/scripts/plan_to_mock.py" > "$SENT/mocks/current-plan.sentinel"

echo "==> sentinel policy check"
{
  cat "$SENT/sentinel.hcl"
  printf '\nmock "tfplan/v2" {\n  module {\n    source = "./mocks/current-plan.sentinel"\n  }\n}\n'
} > "$SENT/gate.hcl"

if (cd "$SENT" && sentinel apply -config=gate.hcl); then
  echo "==> Sentinel gate PASSED"
else
  echo "==> Sentinel gate FAILED: rollout blocked" >&2
  exit 1
fi

if [[ "$APPLY" == "--apply" ]]; then
  echo "==> terraform apply"
  terraform -chdir="$TF" apply -input=false "$WORK/plan.tfplan"
fi
