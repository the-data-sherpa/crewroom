FROM node:24.19.0-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends chromium && rm -rf /var/lib/apt/lists/*
COPY playwright-core /opt/playwright-core
COPY check-ui.cjs /opt/check-ui.cjs
