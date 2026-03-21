# Build stage
FROM node:22-alpine AS builder
WORKDIR /app
COPY package.json ./
RUN npm install
COPY . .
RUN npm run build

# Runtime stage — nginx serves the plugin assets
FROM registry.access.redhat.com/ubi9/ubi-minimal:latest

RUN microdnf install -y nginx && microdnf clean all

COPY --from=builder /app/dist /usr/share/nginx/html

# nginx config for console plugin serving
RUN printf 'server {\n\
    listen 9443 ssl;\n\
    ssl_certificate /var/serving-cert/tls.crt;\n\
    ssl_certificate_key /var/serving-cert/tls.key;\n\
    root /usr/share/nginx/html;\n\
    location / {\n\
        add_header Access-Control-Allow-Origin *;\n\
        try_files $uri $uri/ =404;\n\
    }\n\
}\n' > /etc/nginx/conf.d/default.conf

EXPOSE 9443
USER 1001
CMD ["nginx", "-g", "daemon off;"]
