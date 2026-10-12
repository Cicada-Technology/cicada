//! 权限点覆盖矩阵（M2 出口条件）。
//!
//! 参考实现 80 条权限标识里，18 条 `:view`（C 类）退化为**菜单可见性**、不由 REST 守卫；
//! 其余 62 条（F 类）必须有对应接口。权限点清单的 62 条 F 类**已全部实现**（M3 收尾补齐
//! `tool:gen:*`），此外还有 cicada 自己的扩展命名空间（`CICADA_EXTRA`）。
//! 本测试把「已实现」「C 类菜单」「cicada 扩展」三集合并起来对齐权限点清单，
//! 并保证没有任何越界的新权限点。

use cicada_web::async_export::ExportTaskModule;
use cicada_web::cache::CacheModule;
use cicada_web::codegen::CodeModule;
use cicada_web::data::DataModule;
use cicada_web::file::FileModule;
use cicada_web::jobs::JobModule;
use cicada_web::logs::LogModule;
use cicada_web::module::Module;
use cicada_web::notify::NotifyModule;
use cicada_web::online::OnlineModule;
use cicada_web::server::ServerModule;
use cicada_web::system::SystemModule;

/// 权限点清单的 C 类（`:view`）：仅菜单可见性，不做 REST 守卫。
const APPENDIX_D_MENU: &[&str] = &[
    "system:user:view",
    "system:role:view",
    "system:menu:view",
    "system:dept:view",
    "system:post:view",
    "system:dict:view",
    "system:config:view",
    "system:notice:view",
    "monitor:operlog:view",
    "monitor:logininfor:view",
    "monitor:online:view",
    "monitor:job:view",
    "monitor:data:view",
    "monitor:server:view",
    "monitor:cache:view",
    "tool:build:view",
    "tool:gen:view",
    "tool:swagger:view",
];

/// 权限点清单的 F 类：M2 已实现的部分。
const APPENDIX_D_IMPL: &[&str] = &[
    "system:user:list",
    "system:user:add",
    "system:user:edit",
    "system:user:remove",
    "system:user:export",
    "system:user:import",
    "system:user:resetPwd",
    "system:role:list",
    "system:role:add",
    "system:role:edit",
    "system:role:remove",
    "system:role:export",
    "system:menu:list",
    "system:menu:add",
    "system:menu:edit",
    "system:menu:remove",
    "system:dept:list",
    "system:dept:add",
    "system:dept:edit",
    "system:dept:remove",
    "system:post:list",
    "system:post:add",
    "system:post:edit",
    "system:post:remove",
    "system:post:export",
    "system:dict:list",
    "system:dict:add",
    "system:dict:edit",
    "system:dict:remove",
    "system:dict:export",
    "system:config:list",
    "system:config:add",
    "system:config:edit",
    "system:config:remove",
    "system:config:export",
    "system:notice:list",
    "system:notice:add",
    "system:notice:edit",
    "system:notice:remove",
    "monitor:operlog:list",
    "monitor:operlog:remove",
    "monitor:operlog:detail",
    "monitor:operlog:export",
    "monitor:logininfor:list",
    "monitor:logininfor:remove",
    "monitor:logininfor:export",
    "monitor:logininfor:unlock",
    "monitor:online:list",
    "monitor:online:forceLogout",
    "monitor:online:batchForceLogout",
    "monitor:job:list",
    "monitor:job:add",
    "monitor:job:edit",
    "monitor:job:remove",
    "monitor:job:changeStatus",
    "monitor:job:detail",
    "monitor:job:export",
    "tool:gen:list",
    "tool:gen:edit",
    "tool:gen:remove",
    "tool:gen:preview",
    "tool:gen:code",
];

/// cicada 新增能力（自己的命名空间， 末句允许）。
///
/// `monitor:cache:*` 是缓存监控（缓存监控 / 可观测性）：权限点清单的 `monitor:cache:view` 是 C 类
/// 菜单权限点、不做 REST 守卫，因此观测与清理各自新增一个 F 类权限点，
/// 把「能看缓存」与「能清缓存」分开。
const CICADA_EXTRA: &[&str] = &[
    "system:file:add",
    "system:file:list",
    "system:file:remove",
    "monitor:cache:list",
    "monitor:cache:clear",
    "monitor:server:list",
    "monitor:data:list",
    // 字段级变更留痕（审计设计 P1）。
    "monitor:auditlog:list",
    // 在线会话历史快照（认证设计补记，读 `sys_user_online`）。
    "monitor:online:history",
    // M4：开放接口凭证、租户与套餐、通知中心（自己的命名空间，权限点清单末句允许）。
    "system:apikey:list",
    "system:apikey:add",
    "system:apikey:edit",
    "system:apikey:remove",
    "system:tenant:list",
    "system:tenant:add",
    "system:tenant:edit",
    "system:tenant:remove",
    "system:package:list",
    "system:package:add",
    "system:package:edit",
    "system:package:remove",
    "notify:channel:list",
    "notify:channel:add",
    "notify:channel:edit",
    "notify:channel:remove",
    "notify:template:list",
    "notify:template:add",
    "notify:template:edit",
    "notify:template:remove",
    "notify:record:list",
    "notify:send",
    // 异步导出任务（导入导出）：创建与查询/下载各自的权限点。
    "system:export:list",
    "system:export:add",
];

fn manifest_perms() -> Vec<String> {
    let mut out = Vec::new();
    let modules: Vec<Box<dyn Module>> = vec![
        Box::new(SystemModule),
        Box::new(LogModule),
        Box::new(OnlineModule),
        Box::new(FileModule),
        Box::new(JobModule),
        Box::new(CacheModule),
        Box::new(ServerModule),
        Box::new(DataModule),
        Box::new(CodeModule),
        Box::new(NotifyModule),
        Box::new(ExportTaskModule),
    ];
    for m in modules {
        let manifest = m.manifest();
        for p in &manifest.perms {
            out.push(p.as_str().to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn appendix_d_f_perms_all_accounted_for() {
    let perms = manifest_perms();

    // 1) 已实现的权限点清单权限点必须真的在路由清单里出现。
    for p in APPENDIX_D_IMPL {
        assert!(perms.iter().any(|x| x == p), "已声明实现却缺路由：{p}");
    }

    // 3) C 类 `:view` 一律不做 REST 守卫。
    for p in APPENDIX_D_MENU {
        assert!(
            !perms.iter().any(|x| x == p),
            "C 类菜单权限点不应守卫接口：{p}"
        );
    }

    // 4) 无越界权限点：只允许权限点清单 ∪ cicada 新增命名空间。
    let allowed: Vec<&str> = APPENDIX_D_IMPL
        .iter()
        .chain(APPENDIX_D_MENU.iter())
        .chain(CICADA_EXTRA.iter())
        .copied()
        .collect();
    for p in &perms {
        assert!(
            allowed.contains(&p.as_str()),
            "出现了权限点清单之外的权限点（需登记到 CICADA_EXTRA）：{p}"
        );
    }
}

/// 审计设计之后最容易漏的一环：接口守卫了某个权限点，但种子里没有对应菜单行，
/// 于是那个权限**只能被 admin 的 `*:*:*` 命中**，任何角色都配不出来。
#[test]
fn every_route_perm_is_grantable_from_the_menu_seed() {
    let buttons = cicada_migrations::seed::seeded_button_perms();
    for p in manifest_perms() {
        assert!(
            buttons.contains(&p.as_str()),
            "路由守卫了 `{p}`，但种子没有任何 F 类菜单行授予它"
        );
    }
    // C 类 `:view` 不做守卫，但没有菜单行就不会出现在菜单树里。
    let menus = cicada_migrations::seed::seeded_menu_perms();
    for p in APPENDIX_D_MENU {
        assert!(menus.contains(p), "权限点清单的 C 类菜单没有种子行：{p}");
    }
}

/// 反向对账：种子里的按钮必须真的守卫着某个接口，否则角色树上是个死按钮。
#[test]
fn no_seeded_button_guards_nothing() {
    let perms = manifest_perms();
    for p in cicada_migrations::seed::seeded_button_perms() {
        assert!(
            perms.iter().any(|x| x == p),
            "种子里有按钮 `{p}`，但没有模块清单声明它"
        );
    }
}

#[test]
fn appendix_d_totals_match_spec() {
    assert_eq!(APPENDIX_D_MENU.len(), 18, "C 类应为 18 条");
    assert_eq!(APPENDIX_D_IMPL.len(), 62, "F 类应为 62 条且已全部实现");
    // 权限点清单合计 80 条：18 菜单 + 62 按钮。
    assert_eq!(APPENDIX_D_MENU.len() + APPENDIX_D_IMPL.len(), 80);
}
