# cicada-cli

运维与脚手架命令行：单体二进制，命令少、无嵌套子命令库依赖。

## 职责

```text
cicada-cli migrate [up|down|status]      结构迁移（默认 up）
cicada-cli seed                          字典 / 参数 / 菜单种子（幂等）
cicada-cli init-admin                    默认租户 + admin 账号
cicada-cli tenant init --name <名> [--admin <账号>] [--password <密码>]
cicada-cli admin reset --user <账号> [--tenant <id>] --password <新密码>
cicada-cli rls <enable|disable|status> [--table <表名>]...
cicada-cli gen --table <表名> [--tree] [--out <文件.zip>]   （默认输出 <表名>.zip）
cicada new <目录> [--name <包名>] [--cicada <TOML 片段>] [--template <模板目录>]
```

- `migrate / seed / init-admin / tenant / admin / rls` 直接作用于 `cicada-migrations` 的
  Migrator、种子与 `cicada-infra::rls`（数据权限的 PostgreSQL RLS 兜底按表开启）。
- `gen` 调 `cicada-codegen`，把生成结果打成 zip（代码生成）。
- `new` 把 `template/` 渲染成使用者工程（模板化机制机制 1–2）：五个占位符 + 不落地模板元数据，
  **渲染是失败关闭的**——替换后仍残留 `{{…}}` 直接报错，不产出半成品工程。
  `path=` 模式必须逐 crate 给包目录（cargo 的路径依赖不接受虚拟清单根）。

## 公开 API

没有 lib target，只有 `main.rs`（+ `new.rs`）。**命令面与退出码就是 API**：
出错打印 `USAGE` 并返回非零 `ExitCode`。

## 依赖

清单声明：`cicada`（门面）、`cicada-web` / `cicada-infra` / `cicada-ports` / `cicada-kernel` /
`cicada-migrations` / `cicada-codegen`、`sea-orm` / `sea-orm-migration`、`tokio`、`chrono`。
源码里实际调用的能力是 `cicada-infra`（配置与 RLS）、`cicada-migrations`（Migrator + 种子）、
`cicada-codegen`（生成器），其余是直接依赖（`cargo deny` / `cargo machete` 门禁见，
尚未开启，故未清理，见 `.github/workflows/ci.yml` 顶部说明）。
连接串取自 `config/default.yaml`，或环境变量 `DATABASE_URL`。

## 不做什么

- 不引入 `clap`：参数解析刻意朴素，命令数量少时手写比依赖库更可控。
- 不做交互式 TUI / 长驻进程；一条命令一件事，可被脚本与 CI 直接调用。
- 不复制领域规则：迁移、种子、RLS、生成器的实现分别归属对应 crate，本二进制只做参数与编排。
