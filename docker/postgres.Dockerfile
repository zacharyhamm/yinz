FROM postgres:17-alpine

# Test-only keys stay in the image. xtask exports only the public CA bundle.
# The unrelated CA comes first so tests prove every certificate in the bundle is loaded.
RUN apk add --no-cache openssl \
    && mkdir /tls \
    && openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -subj '/CN=Yinz unused test CA' -addext 'basicConstraints=critical,CA:TRUE' \
        -keyout /tls/unused.key -out /tls/ca-bundle.pem \
    && openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -subj '/CN=Yinz Postgres test CA' -addext 'basicConstraints=critical,CA:TRUE' \
        -keyout /tls/ca.key -out /tls/ca.pem \
    && openssl req -newkey rsa:2048 -nodes -subj '/CN=localhost' \
        -addext 'subjectAltName=DNS:localhost' \
        -keyout /tls/server.key -out /tls/server.csr \
    && openssl x509 -req -in /tls/server.csr -CA /tls/ca.pem -CAkey /tls/ca.key \
        -CAcreateserial -days 3650 -copy_extensions copy -out /tls/server.crt \
    && cat /tls/ca.pem >> /tls/ca-bundle.pem \
    && rm /tls/unused.key /tls/ca.key /tls/ca.srl /tls/server.csr \
    && chown postgres:postgres /tls/server.key \
    && chmod 600 /tls/server.key
