FROM node:24-alpine AS builder
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY tsconfig.json vite.config.ts index.html ./
COPY src ./src
ARG VITE_TROUTE_API_BASE_URL=/api
ARG VITE_TROUTE_ROUTE_PATH=
ARG VITE_TROUTE_COMMIT_SHA=unknown
ENV VITE_TROUTE_COMMIT_SHA=$VITE_TROUTE_COMMIT_SHA
RUN npm run build

FROM nginx:stable-alpine AS runtime
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
ENV TROUTE_PORT=8080
ENV NGINX_ENVSUBST_FILTER=^TROUTE_PORT$
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD wget -q -O /dev/null http://127.0.0.1:8080/ || exit 1
