FROM node:24.19.0-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends chromium && rm -rf /var/lib/apt/lists/*
COPY playwright-core /opt/playwright-core
COPY checks /opt/crewroom-checks
RUN chmod -R a-w /opt/crewroom-checks /opt/playwright-core
WORKDIR /workspace
