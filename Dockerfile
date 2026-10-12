# cicada-server 镜像（「产物：单个二进制」「musl 静态链接优先」）。
#
# 两段式：builder 用 Alpine（musl），产出的二进制不依赖任何 .so；运行段只加
# ca-certificates（出站 TLS：SMTP / 微信 / OIDC）与 curl（健康探针）。
# 生产部署仍是「二进制 + systemd」，镜像服务两件事：
# `docker compose` 的一键开发环境，和一条可复现的构建路径。
#
# 构建（需要 BuildKit，Docker 24+ 默认开）：
#   docker build -t cicada-server:dev .
# 离线/内网环境把 swagger-ui 的 zip 放进 `vendor/` 同名即可，不必访问 GitHub：
#   cp ~/.cache/cicada/swagger-ui-v5.32.6.zip vendor/

# 基础镜像的补丁号必须与 `rust-toolchain.toml` 的 `channel` 逐位相同（含补丁号）：
# `rust:1.98-alpine` 装的是 1.98.1，而清单钉的是 1.98.0，rustup 于是会在构建中途去
# static.rust-lang.org 下载整套工具链——内网/离线环境下这条 RUN 直接炸，
# 报错还是 `tls handshake eof` 这种完全看不出原因的形状。钉死补丁号后构建不再碰网络。
FROM rust:1.98.0-alpine AS builder
RUN apk add --no-cache build-base
WORKDIR /src

# `vendor/` 里只放一个 README 时目录也存在，COPY 不会失败。
COPY vendor/ /tmp/vendor/

# 依赖清单先单独复制：改源码不会让 `cargo fetch` 那一层失效。
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY migrations/ migrations/
COPY apps/ apps/
COPY config/ config/

# zip 在就走离线资产，不在就按 utoipa-swagger-ui 的默认行为去 GitHub 取——
# 不在这里用 ENV 写死，否则缺文件时连回退的机会都没有。
# 部署的两个二进制都在镜像里：`cicada-cli` 用来跑迁移（「先迁移后发版」要求它能与
# 同一个产物一起交付），入口仍是 server。
RUN --mount=type=cache,target=/src/target \
    if [ -f /tmp/vendor/swagger-ui-v5.32.6.zip ]; then \
      export SWAGGER_UI_DOWNLOAD_URL=file:///tmp/vendor/swagger-ui-v5.32.6.zip; \
    fi; \
    cargo build --release -p cicada-server -p cicada-cli \
    && cp /src/target/release/cicada-server /src/target/release/cicada-cli /usr/local/bin/

FROM alpine:3.22
RUN apk add --no-cache ca-certificates curl \
    && adduser -D -u 10001 cicada \
    && mkdir -p /app/data/upload && chown -R cicada /app/data
WORKDIR /app
COPY --from=builder /usr/local/bin/cicada-server /usr/local/bin/cicada-server
COPY --from=builder /usr/local/bin/cicada-cli /usr/local/bin/cicada-cli

# 不把仓库里的 config/ 烘进镜像：`config/default.yaml` 在里是本地文件（已 gitignore），
# 烘进去会让镜像随开发者机器上的改动漂移，而且文件一旦存在，环境变量就再也盖不住端口等了
# （配置的优先级是 env > 文件）。容器要么挂配置目录，要么只给 DATABASE_URL / REDIS_URL。

# 上传与导出产物落这里；镜像本身不留状态（部署「应用实例不保存状态」）。
VOLUME ["/app/data"]
USER cicada
EXPOSE 8080

# 探就绪而不是存活：`/healthz` 进程在就 200，测不出依赖挂了（可观测性）。
HEALTHCHECK --interval=10s --timeout=3s --start-period=15s --retries=5 \
    # 端口可由 CICADA_SERVER_PORT 改（配置的环境变量覆写），探针必须跟着走，
    # 否则换个端口的部署会一直被报成 unhealthy。
    CMD curl -fsS "http://127.0.0.1:${CICADA_SERVER_PORT:-8080}/readyz" || exit 1

ENTRYPOINT ["cicada-server"]
