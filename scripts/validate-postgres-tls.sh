#!/usr/bin/env bash
# Validate supplied material without writing output files or printing key bytes.
set -euo pipefail
if [[ $# -ne 4 ]]; then
  echo "Usage: validate-postgres-tls.sh CERTIFICATE PRIVATE_KEY CA_BUNDLE HOSTNAME" >&2
  exit 2
fi
freight_tls_cert="$1"
freight_tls_key="$2"
freight_tls_ca="$3"
freight_tls_hostname="$4"
for freight_tls_input in "$freight_tls_cert" "$freight_tls_key" "$freight_tls_ca"; do
  if [[ ! -f "$freight_tls_input" || ! -s "$freight_tls_input" || -L "$freight_tls_input" ]]; then
    echo "TLS inputs must be nonempty regular files, without symlinks." >&2
    exit 2
  fi
done
if [[ -z "$freight_tls_hostname" || "$freight_tls_hostname" == -* ]]; then
  echo "A database certificate hostname is required." >&2
  exit 2
fi

# SQLx's Rustls verifier requires a subject alternative name. OpenSSL's hostname
# check alone can accept a legacy Common Name when the DNS SAN is absent.
freight_tls_names="$(openssl x509 -in "$freight_tls_cert" -noout -ext subjectAltName)"
if [[ "$freight_tls_names" != *DNS:* ]]; then
  echo "The database certificate must contain a DNS subject alternative name." >&2
  exit 2
fi

# verify checks validity, chain, hostname and server purpose. The chain file
# starts with the leaf and may also contain its intermediate certificates.
openssl verify -purpose sslserver -verify_hostname "$freight_tls_hostname" \
  -CAfile "$freight_tls_ca" -untrusted "$freight_tls_cert" "$freight_tls_cert" >/dev/null
openssl pkey -in "$freight_tls_key" -passin pass: -check -noout >/dev/null
freight_tls_cert_public="$(openssl x509 -in "$freight_tls_cert" -pubkey -noout |
  openssl pkey -pubin -outform DER | sha256sum)"
freight_tls_key_public="$(openssl pkey -in "$freight_tls_key" -passin pass: -pubout -outform DER | sha256sum)"
if [[ "$freight_tls_cert_public" != "$freight_tls_key_public" ]]; then
  echo "Database certificate and private key do not match." >&2
  exit 2
fi
echo "Database certificate chain, hostname, validity and private key verified."
