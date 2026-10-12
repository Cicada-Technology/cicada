# 前端（Vue 3 + TypeScript + Vite）

独立构建，与 Rust workspace 互不干扰；产物交给 Nginx/CDN 静态托管，后端只出 REST
。设计语言按 Apple 风格，并沉淀成 token 与项目自有组件层。

## 命令

```bash
pnpm install          # 依赖（Node >= 22.12，pnpm 10）
pnpm dev              # 开发服务器 :5173，/api 与 /auth 代理到 VITE_DEV_PROXY_TARGET
pnpm typecheck        # vue-tsc --noEmit
pnpm lint             # ESLint（flat config）
pnpm format           # Prettier
pnpm build            # typecheck + vite build
pnpm preview          # 预览 dist/
```

后端默认跑在 `:8081`（见 `.env.development` 的 `VITE_DEV_PROXY_TARGET`）。开发期用代理而不是
CORS：前端始终按同源相对路径发请求，行为与生产的 Nginx 反代一致。

## 目录

| 路径 | 职责 |
| --- | --- |
| `src/styles/` | 令牌 → reset/工具类 → Element 重皮肤 → 动效，四层顺序固定 |
| `src/api/` | 按后端模块划分的接口层，字段名与后端 DTO 逐字一致（snake_case） |
| `src/utils/request.ts` | 唯一碰 axios 的地方：令牌注入、401 刷新重放、错误归一 |
| `src/stores/` | `app`（外壳偏好）、`auth`（身份与权限）、`permission`（动态路由）、`dict`（字典缓存） |
| `src/router/` | 静态路由、`dynamic.ts`（菜单树 → 路由/侧栏）、守卫 |
| `src/components/` | 项目自有组件（`App*`），Element Plus 只作底层 |
| `src/views/` | 页面；路径与后端菜单的 `path` 对应（`/system/user` → `views/system/user/index.vue`） |
| `src/directives/perm.ts` | `v-perm` / `v-hasPermi` / `v-hasRole` |
| `src/locales/` | 仅前端自身文案；菜单名/表头/字典标签由后端下发 |

## 约定

- **样式**：不写魔法值。颜色、间距、圆角、阴影、字距、时长全部取 `styles/tokens.css`；
  需要新值就先加令牌。Element Plus 通过映射 `--el-*` 到令牌来换肤，不 fork 组件样式。
- **无障碍**：`prefers-reduced-motion` / `-transparency` / `-contrast` 三态在令牌层处理，
  组件不必各自适配。
- **接口**：调用 `utils/request.ts` 的 `get/post/put/delete/upload/download`，返回值是**响应体**
  而不是 axios 响应。集合一律 `{ items, total, page, page_size }`。
- **错误**：所有失败都归一成 `ApiError`；默认由拦截器弹一次全局提示，`silent: true` 可关
  （登录页用它做内联提示）。后端错误体可能是 RFC 9457 的 `problem+json`，也可能是当前的
  `{ "error": "…" }`，`utils/api-error.ts` 两种都能读。
- **状态位语义**（沿用参考实现，与直觉相反）：`status` 0 正常/成功、1 停用/失败；`visible` 0 显示。
- **权限**：`v-perm` 只是界面裁剪，判定永远在后端。权限点字符串与后端
  `#[require_perm]` 逐字一致。
- **表格页**：用 `AppPage` + `AppSearchForm` + `AppTable` + `AppDialog`/`AppDrawer` + `AppForm`
  组合，状态交给 `useTable`，破坏性操作走 `useConfirm`。代码生成器产出的页面用自己的
  `.app-container` 结构，两者视觉由同一套令牌保证一致。
- **组件必须 import**：`App*` 走显式导入（Element Plus 的 `el-*` 由 `app.use` 全量注册）。
  漏引会被 `vue/no-undef-components` 拦下——这不是洁癖：Vue 会把未解析的标签当原生元素渲染，
  `<el-table-column>` 因此找不到所属表格，抛错并打空整个应用（真实踩过一次）。
- **渲染兜底**：页面级错误由 `AppErrorBoundary` 就地接住（侧栏/顶栏还活着，可换页自救），
  根组件再兜一层；它只接渲染/生命周期错误，接口失败仍由拦截器提示，不会把页面替换掉。
- **会话终止只有一条路**：`clearSession('expired')` 广播 → `main.ts::handleSessionLost`
  统一提示并回登录页（带 `?redirect=`）。用户主动登出走 `logout`，不广播也不提示。
  收尾里用 `auth.forget()` 而非 `auth.reset()`，否则会「清会话 → 广播 → 再清会话」递归。
- **动态路由重放**：守卫装载完菜单后重放当前地址时必须写 `{ path: to.path, query: to.query }`，
  写成 `{ path: to.fullPath }` 会把查询串吞掉（vue-router 只认 `rawLocation.query`）。

- 图标使用 [Lucide](https://lucide.dev/icons/)（`@lucide/vue`），由 `AppIcon` 统一解析名称并按需加载。
## 与后端的契约缺口

前端只依赖后端已声明的路由。历史上为两处「接口就绪但前端缺数据源」补过读路径，
现已在后端实现并加了集成测试：

- `GET /api/v1/system/roles/{id}/menus`：角色已授权菜单 id（权限分配界面提交是整体替换，
  读不到原授权就会清空）。
- `GET /api/v1/system/dict/data/by-type/{type}`：登录即可读的字典项（表单下拉框的数据源，
  不要求 `system:dict:list`）。

## 排障

- 侧栏少了整个模块（例如「通知中心」）：种子授权是在该模块加入之前灌的，重跑
  `cargo run -p cicada-cli -- init-admin` 补齐缺失的 `sys_role_menu` 行（幂等，不覆盖已有改动）。
- 某个页面整块变成「页面渲染出错」：控制台有原始报错（边界会打印它再兜住），
  常见原因是该页漏引 `App*` 组件。
- 刷新后查询参数丢了：守卫重放地址时的写法问题，见上面「动态路由重放」。

## 后续批次

富文本编辑器与表单构建（`/tool/build`）尚未实现界面，菜单点进去会落在占位页说明原因
（`views/placeholder/index.vue`）；服务监控与数据监控已实现。
