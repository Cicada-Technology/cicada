# ADR 0004：租户限额落在写路径闸门，用 409 表达「超限」

- 日期：2026-10-10
- 状态：已采纳

## 背景

设计文档 §6.4 要求租户限额（用户数 / 存储量 / 接口调用量）配套 `sys_tenant_package`，
但没有规定在哪些点拦截、以什么 HTTP 语义返回。

## 结论

- `TenantQuota`（`cicada-system`）提供只读用量 + 套餐读取，以及两个闸门：
  `check_user_slot(tenant, additional)`（建号 1、批量导入为行数）与
  `check_storage_slot(tenant, additional_bytes)`（上传前，按本次字节数预检）。
- 调用点：`POST /system/users`、`POST /system/users/import`、`POST /system/files`（上传）。
- 超限返回 **409 Conflict**，与「403 无权限」明确区分：客户端能识别这是套餐上限，
  而不是权限或参数问题。
- `-1` 表示不限；`sys_tenant.package_id` 为空表示无套餐、不限额（`sys_tenant` 表保持可空）。
- 删除套餐前先检查是否仍有租户引用（409）；删除租户前先检查租户内是否还有用户或文件（409）。

## 代价

- `api_qps_limit` 的落地见 ADR 0005：限流层（§3.4 第 7 层）按租户维度调用
  `TenantQuota::api_qps_limit` 解析配额，并做 60s 本地缓存；本 ADR 只管写路径的席位/存储闸门。
- 用量是「当前值 + 本次增量」的乐观判定，未加锁；并发建号可能越过上限 1 个。
  真需要强一致时应在 `sys_tenant` 行上加 `FOR UPDATE` 或改走计数表。
