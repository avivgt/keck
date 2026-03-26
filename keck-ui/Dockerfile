# Build stage
FROM registry.access.redhat.com/ubi9/nodejs-22:latest AS builder
WORKDIR /app
COPY package.json ./
RUN npm install
COPY . .
RUN npm run build

# Runtime stage — nginx serves the plugin assets
FROM registry.access.redhat.com/ubi9/ubi-minimal:latest

RUN microdnf install -y nginx && microdnf clean all

COPY --from=builder /app/dist /usr/share/nginx/html

# nginx config: non-root friendly, TLS for console plugin
RUN printf 'error_log /dev/stderr;\n\
pid /tmp/nginx.pid;\n\
worker_processes auto;\n\
events { worker_connections 1024; }\n\
http {\n\
    include /etc/nginx/mime.types;\n\
    access_log /dev/stdout;\n\
    client_body_temp_path /tmp/client_body;\n\
    proxy_temp_path /tmp/proxy;\n\
    fastcgi_temp_path /tmp/fastcgi;\n\
    uwsgi_temp_path /tmp/uwsgi;\n\
    scgi_temp_path /tmp/scgi;\n\
    server {\n\
        listen 9443 ssl;\n\
        ssl_certificate /var/serving-cert/tls.crt;\n\
        ssl_certificate_key /var/serving-cert/tls.key;\n\
        root /usr/share/nginx/html;\n\
        location / {\n\
            add_header Access-Control-Allow-Origin *;\n\
            try_files $uri $uri/ =404;\n\
        }\n\
    }\n\
}\n' > /etc/nginx/nginx.conf

EXPOSE 9443
USER 1001
CMD ["nginx", "-g", "daemon off;"]
