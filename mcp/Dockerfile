FROM node:22-bookworm-slim
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts && npm cache clean --force
COPY --chown=node:node src ./src
COPY --chown=node:node catalog.json guides.json ./
USER node
ENV NODE_ENV=production
CMD ["node", "src/index.mjs"]
