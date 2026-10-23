# Drop-in replacement for the official caddy:2.11.7 image, plus TLS JA4 (+ JA4H) fingerprint modules.
#   JA4  : github.com/josuebrunel/caddy-ja3ja4 v0.3.0 (commit 1bf7bbde3190), MIT; JA4 via exaring/ja4plus (MIT)
#   JA4H : github.com/lum8rjack/caddy-ja4h @4ad5f179c702 (MIT code; JA4H algorithm under FoxIO License 1.1, non-commercial)
# Build:  docker build -t owl/caddy-ja4:2.11.7-ja4.1 .
FROM caddy:2.11.7-builder AS builder
ARG JA3JA4_VERSION=v0.3.0
ARG JA4H_VERSION=4ad5f179c702c3e9debbb69b027f7748a5f902d2
ARG WITH_JA4H=1
RUN set -eux; \
    extra=""; \
    if [ "$WITH_JA4H" = "1" ]; then extra="--with github.com/lum8rjack/caddy-ja4h@${JA4H_VERSION}"; fi; \
    xcaddy build \
      --with github.com/josuebrunel/caddy-ja3ja4@${JA3JA4_VERSION} \
      $extra

FROM caddy:2.11.7
COPY --from=builder /usr/bin/caddy /usr/bin/caddy
