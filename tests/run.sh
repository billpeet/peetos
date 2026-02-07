#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_DIR="${ROOT_DIR}/tests"

total=0
failed=0

for test in "${TEST_DIR}"/*.sh; do
  case "$(basename "${test}")" in
    run.sh)
      continue
      ;;
  esac

  total=$((total + 1))
  echo "[TEST] $(basename "${test}")"
  if "${test}"; then
    echo "[PASS] $(basename "${test}")"
  else
    echo "[FAIL] $(basename "${test}")"
    failed=$((failed + 1))
  fi
done

if [[ "${total}" -eq 0 ]]; then
  echo "No tests found in ${TEST_DIR}"
  exit 1
fi

if [[ "${failed}" -ne 0 ]]; then
  echo "${failed} test(s) failed"
  exit 1
fi

echo "All ${total} test(s) passed"
