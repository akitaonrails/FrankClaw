FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        chromium \
        ca-certificates \
        fonts-liberation \
        socat \
        curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -m -s /usr/sbin/nologin chrome

COPY docker/chromium/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER chrome

EXPOSE 9222

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -sf http://127.0.0.1:9222/json/version || exit 1

ENTRYPOINT ["/entrypoint.sh"]
