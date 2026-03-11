FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        chromium \
        ca-certificates \
        fonts-liberation \
    && rm -rf /var/lib/apt/lists/*

EXPOSE 9222

ENTRYPOINT ["chromium", "--headless=new", "--disable-gpu", "--disable-dev-shm-usage", "--no-sandbox", "--remote-debugging-address=0.0.0.0", "--remote-debugging-port=9222", "--user-data-dir=/tmp/chromium", "about:blank"]
