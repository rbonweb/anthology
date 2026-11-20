#!/bin/sh
# Generates the test TLS fixtures of testkit::http once; the outputs are committed.
#
#   crates/anthology-testkit/fixtures/tls/generate.sh
#
# openssl runs in the pinned Python 3.12 image (the FROM line of ci/images/python.Dockerfile),
# which ships it. Outputs, in this folder:
#   ca.pem, ca.der   the test CA (RSA 2048, CN "anthology testkit CA"); its key is thrown away
#   leaf.der         the server certificate: DNS localhost, IP 127.0.0.1, IP ::1, serverAuth
#   leaf-key.der     the server's private key, PKCS#8 DER. Never PEM: the repository runs
#                    gitleaks, and fixture secrets are assembled at run time (rust-lane
#                    protocol), so a test that needs a PEM key writes one into its sandbox.
# Both certificates are valid for 100 years. Apple's platform verifier refuses a server
# certificate valid for more than 825 days, so on macOS trust the CA through a custom root store
# (`TestServer::ca_der`) or a CA file, never the system verifier.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
image=$(awk '$1 == "FROM" {print $2; exit}' "$root/ci/images/python.Dockerfile")

docker run --rm -v "$here":/out -w /tmp "$image" sh -ec '
    openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 36500 -sha256 \
        -subj "/CN=anthology testkit CA" \
        -addext "basicConstraints=critical,CA:TRUE" \
        -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
    openssl req -newkey rsa:2048 -nodes -keyout leaf.key -out leaf.csr \
        -subj "/CN=localhost" 2>/dev/null
    printf "%s\n" "basicConstraints=critical,CA:FALSE" \
        "keyUsage=critical,digitalSignature,keyEncipherment" \
        "extendedKeyUsage=serverAuth" \
        "subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1" > leaf.ext
    openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 36500 \
        -sha256 -extfile leaf.ext -out leaf.pem 2>/dev/null
    openssl verify -CAfile ca.pem leaf.pem
    cp ca.pem /out/ca.pem
    openssl x509 -in ca.pem -outform DER -out /out/ca.der
    openssl x509 -in leaf.pem -outform DER -out /out/leaf.der
    openssl pkcs8 -topk8 -nocrypt -in leaf.key -outform DER -out /out/leaf-key.der
'
echo "generate.sh: wrote ca.pem, ca.der, leaf.der and leaf-key.der"
