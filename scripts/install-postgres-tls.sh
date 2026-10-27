#!/usr/bin/env bash
# Publish a complete validated generation to an owned TLS volume. Run with
# PostgreSQL stopped; the Compose helper defaults to the PostgreSQL image user.
set -euo pipefail
freight_tls_cert=/source/server.crt
freight_tls_key=/source/server.key
freight_tls_ca=/source/ca.crt
freight_tls_destination=/tls
freight_tls_owner=postgres:postgres
freight_tls_hostname=postgres
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cert|--key|--ca|--destination|--owner|--hostname)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "Every TLS installer option requires a value." >&2
        exit 2
      fi
      case "$1" in
        --cert) freight_tls_cert="$2" ;;
        --key) freight_tls_key="$2" ;;
        --ca) freight_tls_ca="$2" ;;
        --destination) freight_tls_destination="$2" ;;
        --owner) freight_tls_owner="$2" ;;
        --hostname) freight_tls_hostname="$2" ;;
      esac
      shift 2
      ;;
    *) echo "Unknown TLS installer option." >&2; exit 2 ;;
  esac
done
freight_tls_script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
bash "$freight_tls_script_dir/validate-postgres-tls.sh" \
  "$freight_tls_cert" "$freight_tls_key" "$freight_tls_ca" "$freight_tls_hostname"

if [[ ! -d "$freight_tls_destination" || -L "$freight_tls_destination" ]]; then
  echo "TLS destination must be an existing owned directory, without a symlink." >&2
  exit 2
fi
freight_tls_destination="$(cd -- "$freight_tls_destination" && pwd -P)"
if [[ -L "$freight_tls_destination/current" ]]; then
  freight_tls_previous="$(readlink -- "$freight_tls_destination/current")"
  if [[ ! "$freight_tls_previous" =~ ^material\.[[:alnum:]]+$ ||
        ! -d "$freight_tls_destination/$freight_tls_previous" ||
        -L "$freight_tls_destination/$freight_tls_previous" ]]; then
    echo "Existing TLS publication is not a managed generation." >&2
    exit 2
  fi
elif [[ -e "$freight_tls_destination/current" ]]; then
  echo "Refusing to replace an unmanaged TLS publication." >&2
  exit 2
fi

freight_tls_staging="$(mktemp -d "$freight_tls_destination/material.XXXXXXXXXX")"
freight_tls_link="$freight_tls_destination/.current-${freight_tls_staging##*/}"
freight_tls_cleanup() {
  # A signal immediately after the rename must not remove the published target.
  if [[ "$(readlink -- "$freight_tls_destination/current" 2>/dev/null || true)" != "${freight_tls_staging##*/}" ]]; then
    rm -rf -- "$freight_tls_staging"
  fi
  rm -f -- "$freight_tls_link"
}
trap freight_tls_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
install -m 0644 -- "$freight_tls_cert" "$freight_tls_staging/server.crt"
install -m 0600 -- "$freight_tls_key" "$freight_tls_staging/server.key"
install -m 0644 -- "$freight_tls_ca" "$freight_tls_staging/ca.crt"
# Validate the staged bytes too, so a changed source cannot publish a mixed pair.
bash "$freight_tls_script_dir/validate-postgres-tls.sh" \
  "$freight_tls_staging/server.crt" "$freight_tls_staging/server.key" \
  "$freight_tls_staging/ca.crt" "$freight_tls_hostname"
chmod 0700 -- "$freight_tls_staging"
chown -R -- "$freight_tls_owner" "$freight_tls_staging"
ln -s -- "${freight_tls_staging##*/}" "$freight_tls_link"
mv -Tf -- "$freight_tls_link" "$freight_tls_destination/current"
echo "Complete database TLS generation published with a private mode-0600 key."
