use sea_orm_migration::sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement,
};
use sea_orm_migration::sea_query::{Alias, Expr, OnConflict, Query, Value};

/// 把任意可 Into<Value> 的值统一转成 Value，消除类型推断歧义。
fn v<T: Into<Value>>(x: T) -> Expr {
    Expr::Value(x.into())
}

async fn insert_row(
    db: &DatabaseConnection,
    table: &str,
    _pk: &str,
    columns: &[&str],
    values: Vec<Expr>,
) -> Result<(), DbErr> {
    let cols: Vec<Alias> = columns.iter().map(|c| Alias::new(*c)).collect();
    let insert = Query::insert()
        .into_table(Alias::new(table))
        .columns(cols)
        .values_panic(values)
        .on_conflict(OnConflict::new().do_nothing().to_owned())
        .to_owned();
    db.execute(&insert).await?;
    Ok(())
}

/// 字典 / 参数 / 菜单 / 框架任务种子（幂等 upsert：冲突即跳过）。
pub async fn seed(db: &DatabaseConnection) -> Result<(), DbErr> {
    seed_dict_types(db).await?;
    seed_dict_data(db).await?;
    seed_config(db).await?;
    seed_menus(db).await?;
    seed_jobs(db).await?;
    seed_notify_templates(db, DEFAULT_TENANT).await?;
    reset_sequences(db).await?;
    Ok(())
}

/// 框架自带的通知模板（大表走异步任务，完成后经通知中心给下载链接）。
///
/// 为什么在种子而不在运行期「缺了现补」：模板的键是 `(tenant_id, code, channel_kind, locale)`，
/// 是**租户级数据**，只有跟着建租户的那条路一起写才不会漏；而渠道（`sys_notify_channel`）
/// 带着 SMTP / webhook 的端点与凭据，按配置「敏感值一律来自环境变量或密钥管理」的边界
/// **不进种子**——运营在 `/api/v1/notify/channels` 建一条 `kind=email` 的渠道即可。
/// 没有渠道时导出只少发一条通知，任务照常完成、列表页照常能下载。
///
/// 链接写的是 API 路径而不是绝对 URL：基座没有「对外可访问地址」这一项配置
/// （配置），凭猜一个 host 反而更容易点不开。
pub async fn seed_notify_templates(db: &DatabaseConnection, tenant_id: i64) -> Result<(), DbErr> {
    const COLS: &[&str] = &[
        "tenant_id",
        "code",
        "name",
        "channel_kind",
        "locale",
        "title",
        "body",
        "create_by",
        "create_time",
    ];
    let now = chrono::Utc::now();
    for (locale, name, title, body) in [
        (
            "zh-CN",
            "异步导出完成",
            "导出完成：{{ module }}",
            "任务 #{{ task_id }}（{{ module }}）已完成，共 {{ processed }} 行。\n\
             下载：/api/v1/system/export-tasks/{{ task_id }}/download",
        ),
        (
            "en-US",
            "Async export finished",
            "Export finished: {{ module }}",
            "Task #{{ task_id }} ({{ module }}) finished with {{ processed }} row(s).\n\
             Download: /api/v1/system/export-tasks/{{ task_id }}/download",
        ),
    ] {
        insert_row(
            db,
            "sys_notify_template",
            "id",
            COLS,
            vec![
                v(tenant_id),
                v(EXPORT_DONE_TEMPLATE_CODE),
                v(name),
                v("email"),
                v(locale),
                v(title),
                v(body),
                v("system"),
                v(now),
            ],
        )
        .await?;
    }
    Ok(())
}

/// 导出完成通知的模板码，生产者（`cicada-web` 的异步导出）与种子读同一个常量。
pub const EXPORT_DONE_TEMPLATE_CODE: &str = "export.done";

/// 默认租户（待确认项 #1：`tenant.enabled = true` 但默认单租户）。
const DEFAULT_TENANT: i64 = 1;

/// 种子 / 初始化数据显式写了主键，序列不会自动前进；不重置则后续自增插入必冲突。
/// 只覆盖「显式写 id」的表。
pub async fn reset_sequences(db: &DatabaseConnection) -> Result<(), DbErr> {
    const TABLES: &[(&str, &str)] = &[
        ("sys_tenant", "tenant_id"),
        ("sys_role", "role_id"),
        ("sys_user", "user_id"),
        ("sys_user_identity", "identity_id"),
        ("sys_menu", "menu_id"),
        ("sys_dict_type", "dict_id"),
        ("sys_dict_data", "dict_code"),
        ("sys_config", "config_id"),
        ("sys_job", "job_id"),
    ];
    for (table, col) in TABLES {
        reset_sequence(db, table, col).await?;
    }
    Ok(())
}

async fn reset_sequence(db: &DatabaseConnection, table: &str, col: &str) -> Result<(), DbErr> {
    let sql = format!(
        "SELECT setval(pg_get_serial_sequence('{table}', '{col}'), \
         COALESCE((SELECT MAX({col}) FROM {table}), 0) + 1, false)"
    );
    db.execute_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await?;
    Ok(())
}

async fn seed_dict_types(db: &DatabaseConnection) -> Result<(), DbErr> {
    const COLS: &[&str] = &[
        "dict_id",
        "dict_name",
        "dict_type",
        "status",
        "create_by",
        "create_time",
        "update_by",
        "update_time",
        "remark",
    ];
    let rows: &[(i64, &str, &str)] = &[
        (1, "用户性别", "sys_user_sex"),
        (2, "菜单状态", "sys_show_hide"),
        (3, "系统开关", "sys_normal_disable"),
        (4, "任务状态", "sys_job_status"),
        (5, "任务分组", "sys_job_group"),
        (6, "系统是否", "sys_yes_no"),
        (7, "通知类型", "sys_notice_type"),
        (8, "通知状态", "sys_notice_status"),
        (9, "操作类型", "sys_oper_type"),
        (10, "系统状态", "sys_common_status"),
    ];
    for (id, name, dtype) in rows {
        insert_row(
            db,
            "sys_dict_type",
            "dict_id",
            COLS,
            vec![
                v(*id),
                v(*name),
                v(*dtype),
                v(0i16),
                v("admin"),
                v(chrono::Utc::now()),
                v(Value::String(None)),
                v(Value::ChronoDateTimeUtc(None)),
                v(*name),
            ],
        )
        .await?;
    }
    Ok(())
}

async fn seed_dict_data(db: &DatabaseConnection) -> Result<(), DbErr> {
    const COLS: &[&str] = &[
        "dict_code",
        "dict_sort",
        "dict_label",
        "dict_value",
        "dict_type",
        "css_class",
        "list_class",
        "is_default",
        "status",
        "create_by",
        "create_time",
        "update_by",
        "update_time",
        "remark",
    ];
    /// 一行字典数据：`(dict_code, dict_sort, label, value, type, css_class, list_class, is_default)`。
    type DictRow<'a> = (
        i64,
        i32,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
    );
    let rows: &[DictRow] = &[
        (1, 1, "男", "0", "sys_user_sex", "", "", "Y"),
        (2, 2, "女", "1", "sys_user_sex", "", "", "N"),
        (3, 3, "未知", "2", "sys_user_sex", "", "", "N"),
        (4, 1, "显示", "0", "sys_show_hide", "", "primary", "Y"),
        (5, 2, "隐藏", "1", "sys_show_hide", "", "danger", "N"),
        (6, 1, "正常", "0", "sys_normal_disable", "", "primary", "Y"),
        (7, 2, "停用", "1", "sys_normal_disable", "", "danger", "N"),
        (8, 1, "正常", "0", "sys_job_status", "", "primary", "Y"),
        (9, 2, "暂停", "1", "sys_job_status", "", "danger", "N"),
        (10, 1, "默认", "DEFAULT", "sys_job_group", "", "", "Y"),
        (11, 2, "系统", "SYSTEM", "sys_job_group", "", "", "N"),
        (12, 1, "是", "Y", "sys_yes_no", "", "primary", "Y"),
        (13, 2, "否", "N", "sys_yes_no", "", "danger", "N"),
        (14, 1, "通知", "1", "sys_notice_type", "", "warning", "Y"),
        (15, 2, "公告", "2", "sys_notice_type", "", "success", "N"),
        (16, 1, "正常", "0", "sys_notice_status", "", "primary", "Y"),
        (17, 2, "关闭", "1", "sys_notice_status", "", "danger", "N"),
        (18, 99, "其他", "0", "sys_oper_type", "", "info", "N"),
        (19, 1, "新增", "1", "sys_oper_type", "", "info", "N"),
        (20, 2, "修改", "2", "sys_oper_type", "", "info", "N"),
        (21, 3, "删除", "3", "sys_oper_type", "", "danger", "N"),
        (22, 4, "授权", "4", "sys_oper_type", "", "primary", "N"),
        (23, 5, "导出", "5", "sys_oper_type", "", "warning", "N"),
        (24, 6, "导入", "6", "sys_oper_type", "", "warning", "N"),
        (25, 7, "强退", "7", "sys_oper_type", "", "danger", "N"),
        (26, 8, "生成代码", "8", "sys_oper_type", "", "warning", "N"),
        (27, 9, "清空数据", "9", "sys_oper_type", "", "danger", "N"),
        (28, 1, "成功", "0", "sys_common_status", "", "primary", "N"),
        (29, 2, "失败", "1", "sys_common_status", "", "danger", "N"),
    ];
    for (code, sort, label, value, dtype, css, list, is_default) in rows {
        insert_row(
            db,
            "sys_dict_data",
            "dict_code",
            COLS,
            vec![
                v(*code),
                v(*sort),
                v(*label),
                v(*value),
                v(*dtype),
                v(*css),
                v(*list),
                v(*is_default),
                v(0i16),
                v("admin"),
                v(chrono::Utc::now()),
                v(Value::String(None)),
                v(Value::ChronoDateTimeUtc(None)),
                v(*label),
            ],
        )
        .await?;
    }
    Ok(())
}

async fn seed_config(db: &DatabaseConnection) -> Result<(), DbErr> {
    const COLS: &[&str] = &[
        "config_id",
        "config_name",
        "config_key",
        "config_value",
        "config_type",
        "create_by",
        "create_time",
        "update_by",
        "update_time",
        "remark",
    ];
    let rows: &[(i64, &str, &str, &str, &str)] = &[
        (
            1,
            "主框架页-默认皮肤样式名称",
            "sys.index.skinName",
            "skin-blue",
            "蓝色 skin-blue、绿色 skin-green、紫色 skin-purple、红色 skin-red、黄色 skin-yellow",
        ),
        (
            2,
            "用户管理-账号初始密码",
            "sys.user.initPassword",
            "Cicada@123",
            "新用户默认密码，须符合口令规则（≥8 位 + 大小写 + 数字）；导入前会校验，\n参考实现的 123456 不合规所以换了",
        ),
        (
            3,
            "主框架页-侧边栏主题",
            "sys.index.sideTheme",
            "theme-dark",
            "深黑主题theme-dark，浅色主题theme-light，深蓝主题theme-blue",
        ),
        (
            4,
            "账号自助-是否开启用户注册功能",
            "sys.account.registerUser",
            "false",
            "是否开启注册用户功能（true开启，false关闭）",
        ),
        (
            5,
            "用户管理-密码字符范围",
            "sys.account.chrtype",
            "3",
            "参考实现兼容：1数字 2字母 3字母和数字；cicada 实际规则为至少大写+小写+数字各一",
        ),
        (
            6,
            "用户管理-初始密码修改策略",
            "sys.account.initPasswordModify",
            "1",
            "0关闭 1提醒修改初始密码",
        ),
        (
            7,
            "用户管理-账号密码更新周期",
            "sys.account.passwordValidateDays",
            "0",
            "密码更新周期（0不限制）",
        ),
        (
            8,
            "主框架页-菜单导航显示风格",
            "sys.index.menuStyle",
            "default",
            "菜单导航显示风格",
        ),
        (
            9,
            "主框架页-是否开启页脚",
            "sys.index.footer",
            "true",
            "是否开启底部页脚",
        ),
        (
            10,
            "主框架页-是否开启页签",
            "sys.index.tagsView",
            "true",
            "是否开启菜单多页签",
        ),
        (
            11,
            "用户登录-黑名单列表",
            "sys.login.blackIPList",
            "",
            "登录IP黑名单，多个以;分隔",
        ),
    ];
    for (id, name, key, value, remark) in rows {
        insert_row(
            db,
            "sys_config",
            "config_id",
            COLS,
            vec![
                v(*id),
                v(*name),
                v(*key),
                v(*value),
                v("Y"),
                v("admin"),
                v(chrono::Utc::now()),
                v(Value::String(None)),
                v(Value::ChronoDateTimeUtc(None)),
                v(*remark),
            ],
        )
        .await?;
    }
    Ok(())
}

/// 菜单种子行的唯一事实来源（+ cicada 扩展）。
///
/// 提到模块级 `pub` 是为了让权限覆盖测试能直接比对「路由声明的权限点」与
/// 「落库的菜单行」——缺任一侧都会造成「接口守卫了一个无法被授予的权限」，
/// 而那种权限只有 admin 的 `*:*:*` 能用。
///
/// `(menu_id, parent_id, menu_name, order_num, menu_type, path, perms)`
pub const MENU_SEED: &[(i64, i64, &str, i32, &str, &str, &str)] = &[
    (1, 0, "系统管理", 1, "M", "#", ""),
    (2, 0, "系统监控", 2, "M", "#", ""),
    (3, 0, "系统工具", 3, "M", "#", ""),
    (4, 0, "Rust 官网", 4, "C", "https://www.rust-lang.org/", ""),
    (
        100,
        1,
        "用户管理",
        1,
        "C",
        "/system/user",
        "system:user:view",
    ),
    (
        101,
        1,
        "角色管理",
        2,
        "C",
        "/system/role",
        "system:role:view",
    ),
    (
        102,
        1,
        "菜单管理",
        3,
        "C",
        "/system/menu",
        "system:menu:view",
    ),
    (
        103,
        1,
        "部门管理",
        4,
        "C",
        "/system/dept",
        "system:dept:view",
    ),
    (
        104,
        1,
        "岗位管理",
        5,
        "C",
        "/system/post",
        "system:post:view",
    ),
    (
        105,
        1,
        "字典管理",
        6,
        "C",
        "/system/dict",
        "system:dict:view",
    ),
    (
        106,
        1,
        "参数设置",
        7,
        "C",
        "/system/config",
        "system:config:view",
    ),
    (
        107,
        1,
        "通知公告",
        8,
        "C",
        "/system/notice",
        "system:notice:view",
    ),
    (108, 1, "日志管理", 9, "M", "#", ""),
    (
        109,
        2,
        "在线用户",
        1,
        "C",
        "/monitor/online",
        "monitor:online:view",
    ),
    (
        110,
        2,
        "定时任务",
        2,
        "C",
        "/monitor/job",
        "monitor:job:view",
    ),
    (
        111,
        2,
        "数据监控",
        3,
        "C",
        "/monitor/data",
        "monitor:data:view",
    ),
    (
        112,
        2,
        "服务监控",
        4,
        "C",
        "/monitor/server",
        "monitor:server:view",
    ),
    (
        113,
        2,
        "缓存监控",
        5,
        "C",
        "/monitor/cache",
        "monitor:cache:view",
    ),
    (114, 3, "表单构建", 1, "C", "/tool/build", "tool:build:view"),
    (115, 3, "代码生成", 2, "C", "/tool/gen", "tool:gen:view"),
    (
        116,
        3,
        "系统接口",
        3,
        "C",
        "/tool/swagger",
        "tool:swagger:view",
    ),
    (
        500,
        108,
        "操作日志",
        1,
        "C",
        "/monitor/operlog",
        "monitor:operlog:view",
    ),
    (
        501,
        108,
        "登录日志",
        2,
        "C",
        "/monitor/logininfor",
        "monitor:logininfor:view",
    ),
    (
        502,
        108,
        "变更留痕",
        3,
        "C",
        "/monitor/auditlog",
        "monitor:auditlog:view",
    ),
    // 用户管理按钮
    (1000, 100, "用户查询", 1, "F", "#", "system:user:list"),
    (1001, 100, "用户新增", 2, "F", "#", "system:user:add"),
    (1002, 100, "用户修改", 3, "F", "#", "system:user:edit"),
    (1003, 100, "用户删除", 4, "F", "#", "system:user:remove"),
    (1004, 100, "用户导出", 5, "F", "#", "system:user:export"),
    (1005, 100, "用户导入", 6, "F", "#", "system:user:import"),
    (1006, 100, "重置密码", 7, "F", "#", "system:user:resetPwd"),
    // 角色管理按钮
    (1007, 101, "角色查询", 1, "F", "#", "system:role:list"),
    (1008, 101, "角色新增", 2, "F", "#", "system:role:add"),
    (1009, 101, "角色修改", 3, "F", "#", "system:role:edit"),
    (1010, 101, "角色删除", 4, "F", "#", "system:role:remove"),
    (1011, 101, "角色导出", 5, "F", "#", "system:role:export"),
    // 菜单管理按钮
    (1012, 102, "菜单查询", 1, "F", "#", "system:menu:list"),
    (1013, 102, "菜单新增", 2, "F", "#", "system:menu:add"),
    (1014, 102, "菜单修改", 3, "F", "#", "system:menu:edit"),
    (1015, 102, "菜单删除", 4, "F", "#", "system:menu:remove"),
    // 部门管理按钮
    (1016, 103, "部门查询", 1, "F", "#", "system:dept:list"),
    (1017, 103, "部门新增", 2, "F", "#", "system:dept:add"),
    (1018, 103, "部门修改", 3, "F", "#", "system:dept:edit"),
    (1019, 103, "部门删除", 4, "F", "#", "system:dept:remove"),
    // 岗位管理按钮
    (1020, 104, "岗位查询", 1, "F", "#", "system:post:list"),
    (1021, 104, "岗位新增", 2, "F", "#", "system:post:add"),
    (1022, 104, "岗位修改", 3, "F", "#", "system:post:edit"),
    (1023, 104, "岗位删除", 4, "F", "#", "system:post:remove"),
    (1024, 104, "岗位导出", 5, "F", "#", "system:post:export"),
    // 字典管理按钮
    (1025, 105, "字典查询", 1, "F", "#", "system:dict:list"),
    (1026, 105, "字典新增", 2, "F", "#", "system:dict:add"),
    (1027, 105, "字典修改", 3, "F", "#", "system:dict:edit"),
    (1028, 105, "字典删除", 4, "F", "#", "system:dict:remove"),
    (1029, 105, "字典导出", 5, "F", "#", "system:dict:export"),
    // 参数设置按钮
    (1030, 106, "参数查询", 1, "F", "#", "system:config:list"),
    (1031, 106, "参数新增", 2, "F", "#", "system:config:add"),
    (1032, 106, "参数修改", 3, "F", "#", "system:config:edit"),
    (1033, 106, "参数删除", 4, "F", "#", "system:config:remove"),
    (1034, 106, "参数导出", 5, "F", "#", "system:config:export"),
    // 通知公告按钮
    (1035, 107, "公告查询", 1, "F", "#", "system:notice:list"),
    (1036, 107, "公告新增", 2, "F", "#", "system:notice:add"),
    (1037, 107, "公告修改", 3, "F", "#", "system:notice:edit"),
    (1038, 107, "公告删除", 4, "F", "#", "system:notice:remove"),
    // 操作日志按钮
    (1039, 500, "操作查询", 1, "F", "#", "monitor:operlog:list"),
    (1040, 500, "操作删除", 2, "F", "#", "monitor:operlog:remove"),
    (1041, 500, "详细信息", 3, "F", "#", "monitor:operlog:detail"),
    (1042, 500, "日志导出", 4, "F", "#", "monitor:operlog:export"),
    // 登录日志按钮
    (
        1043,
        501,
        "登录查询",
        1,
        "F",
        "#",
        "monitor:logininfor:list",
    ),
    (
        1044,
        501,
        "登录删除",
        2,
        "F",
        "#",
        "monitor:logininfor:remove",
    ),
    (
        1045,
        501,
        "日志导出",
        3,
        "F",
        "#",
        "monitor:logininfor:export",
    ),
    (
        1046,
        501,
        "账户解锁",
        4,
        "F",
        "#",
        "monitor:logininfor:unlock",
    ),
    // 在线用户按钮
    (1047, 109, "在线查询", 1, "F", "#", "monitor:online:list"),
    (
        1048,
        109,
        "批量强退",
        2,
        "F",
        "#",
        "monitor:online:batchForceLogout",
    ),
    (
        1049,
        109,
        "单条强退",
        3,
        "F",
        "#",
        "monitor:online:forceLogout",
    ),
    // 定时任务按钮
    (1050, 110, "任务查询", 1, "F", "#", "monitor:job:list"),
    (1051, 110, "任务新增", 2, "F", "#", "monitor:job:add"),
    (1052, 110, "任务修改", 3, "F", "#", "monitor:job:edit"),
    (1053, 110, "任务删除", 4, "F", "#", "monitor:job:remove"),
    (
        1054,
        110,
        "状态修改",
        5,
        "F",
        "#",
        "monitor:job:changeStatus",
    ),
    (1055, 110, "任务详细", 6, "F", "#", "monitor:job:detail"),
    (1056, 110, "任务导出", 7, "F", "#", "monitor:job:export"),
    // 代码生成按钮
    (1057, 115, "生成查询", 1, "F", "#", "tool:gen:list"),
    (1058, 115, "生成修改", 2, "F", "#", "tool:gen:edit"),
    (1059, 115, "生成删除", 3, "F", "#", "tool:gen:remove"),
    (1060, 115, "预览代码", 4, "F", "#", "tool:gen:preview"),
    (1061, 115, "生成代码", 5, "F", "#", "tool:gen:code"),
    // ---- cicada 扩展（M4）：平台管理（租户/套餐/开放接口）----
    (5, 0, "平台管理", 5, "M", "#", ""),
    (
        200,
        5,
        "租户管理",
        1,
        "C",
        "/system/tenant",
        "system:tenant:view",
    ),
    (
        201,
        5,
        "租户套餐",
        2,
        "C",
        "/system/package",
        "system:package:view",
    ),
    (
        202,
        5,
        "开放接口",
        3,
        "C",
        "/system/apikey",
        "system:apikey:view",
    ),
    (1062, 200, "租户查询", 1, "F", "#", "system:tenant:list"),
    (1063, 200, "租户新增", 2, "F", "#", "system:tenant:add"),
    (1064, 200, "租户修改", 3, "F", "#", "system:tenant:edit"),
    (1065, 200, "租户删除", 4, "F", "#", "system:tenant:remove"),
    (1066, 201, "套餐查询", 1, "F", "#", "system:package:list"),
    (1067, 201, "套餐新增", 2, "F", "#", "system:package:add"),
    (1068, 201, "套餐修改", 3, "F", "#", "system:package:edit"),
    (1069, 201, "套餐删除", 4, "F", "#", "system:package:remove"),
    (1070, 202, "凭证查询", 1, "F", "#", "system:apikey:list"),
    (1071, 202, "凭证签发", 2, "F", "#", "system:apikey:add"),
    (1072, 202, "凭证修改", 3, "F", "#", "system:apikey:edit"),
    (1073, 202, "凭证删除", 4, "F", "#", "system:apikey:remove"),
    // ---- cicada 扩展（M4）：通知中心 ----
    (6, 0, "通知中心", 6, "M", "#", ""),
    (
        300,
        6,
        "通知渠道",
        1,
        "C",
        "/notify/channel",
        "notify:channel:view",
    ),
    (
        301,
        6,
        "通知模板",
        2,
        "C",
        "/notify/template",
        "notify:template:view",
    ),
    (
        302,
        6,
        "发送记录",
        3,
        "C",
        "/notify/record",
        "notify:record:view",
    ),
    (1074, 300, "渠道查询", 1, "F", "#", "notify:channel:list"),
    (1075, 300, "渠道新增", 2, "F", "#", "notify:channel:add"),
    (1076, 300, "渠道修改", 3, "F", "#", "notify:channel:edit"),
    (1077, 300, "渠道删除", 4, "F", "#", "notify:channel:remove"),
    (1078, 301, "模板查询", 1, "F", "#", "notify:template:list"),
    (1079, 301, "模板新增", 2, "F", "#", "notify:template:add"),
    (1080, 301, "模板修改", 3, "F", "#", "notify:template:edit"),
    (1081, 301, "模板删除", 4, "F", "#", "notify:template:remove"),
    (1082, 302, "记录查询", 1, "F", "#", "notify:record:list"),
    (1083, 302, "测试发送", 2, "F", "#", "notify:send"),
    // ---- cicada 扩展：把「只有路由声明、没有菜单行」的权限点补齐 ----
    // 没有这些行，权限就只能靠 admin 的 `*:*:*` 兜底，其它角色无论怎么配角色树都拿不到。
    (
        203,
        1,
        "文件管理",
        10,
        "C",
        "/system/file",
        "system:file:view",
    ),
    (
        204,
        1,
        "导出任务",
        11,
        "C",
        "/system/export",
        "system:export:view",
    ),
    // 服务监控 / 缓存监控的只读与清理按钮（可观测性的两个监控页）。
    (1084, 112, "服务信息", 1, "F", "#", "monitor:server:list"),
    // 字段级变更留痕（审计设计，P1）。
    (1092, 502, "留痕查询", 1, "F", "#", "monitor:auditlog:list"),
    (1085, 113, "缓存查询", 1, "F", "#", "monitor:cache:list"),
    (1086, 113, "缓存清理", 2, "F", "#", "monitor:cache:clear"),
    // 数据监控：只读数据库统计（菜单 `monitor:data:view` 只负责可见性）。
    (1094, 111, "数据查询", 1, "F", "#", "monitor:data:list"),
    // 文件存储（文件存储）与异步导出（导入导出）。
    (1087, 203, "文件查询", 1, "F", "#", "system:file:list"),
    (1088, 203, "文件上传", 2, "F", "#", "system:file:add"),
    (1089, 203, "文件删除", 3, "F", "#", "system:file:remove"),
    (1090, 204, "任务查询", 1, "F", "#", "system:export:list"),
    (1091, 204, "任务提交", 2, "F", "#", "system:export:add"),
    (
        1093,
        109,
        "会话历史查询",
        4,
        "F",
        "#",
        "monitor:online:history",
    ),
];

/// 种子里出现过的全部权限标识（去重、非空）。测试用它反查「有菜单但没接口」。
pub fn seeded_perms() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = MENU_SEED
        .iter()
        .map(|r| r.6)
        .filter(|p| !p.is_empty())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// 种子里 F 类（按钮）的权限标识。
pub fn seeded_button_perms() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = MENU_SEED
        .iter()
        .filter(|r| r.4 == "F")
        .map(|r| r.6)
        .filter(|p| !p.is_empty())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// 种子里 C 类（菜单）的权限标识。
pub fn seeded_menu_perms() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = MENU_SEED
        .iter()
        .filter(|r| r.4 == "C")
        .map(|r| r.6)
        .filter(|p| !p.is_empty())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

async fn seed_menus(db: &DatabaseConnection) -> Result<(), DbErr> {
    const COLS: &[&str] = &[
        "menu_id",
        "menu_name",
        "parent_id",
        "order_num",
        "path",
        "component",
        "query",
        "is_frame",
        "is_cache",
        "menu_type",
        "visible",
        "status",
        "perms",
        "icon",
        "remark",
        "create_by",
        "create_time",
        "update_by",
        "update_time",
    ];
    let rows: &[(i64, i64, &str, i32, &str, &str, &str)] = MENU_SEED;
    for (id, parent, name, order, mtype, path, perms) in rows {
        insert_row(
            db,
            "sys_menu",
            "menu_id",
            COLS,
            vec![
                v(*id),
                v(*name),
                v(*parent),
                v(*order),
                v(*path),
                v(""),
                v(Value::String(None)),
                v(1i16),
                v(0i16),
                v(*mtype),
                v(0i16),
                v(0i16),
                v(*perms),
                v(""),
                v(*name),
                v("admin"),
                v(chrono::Utc::now()),
                v(Value::String(None)),
                v(Value::ChronoDateTimeUtc(None)),
            ],
        )
        .await?;
    }
    Ok(())
}

/// 框架自带任务：日志保留期与分区表的自动建分区。
///
/// `register_fn` 只挂上 handler，调度看的是 `sys_job` 行——没有这一行，日志分区窗口
/// 不会自己往前铺，到期分区也永远不会归档。`do_nothing` 是有意的：运维把它暂停或改名后，
/// 重新 seed 不该把它强行恢复。
/// 框架自带任务的调用目标（`sys_job.invoke_target`）。
///
/// 目标字符串的另一端是组装根里 `register_fn` 的 key；两处各写一份字面量迟早会漂，
/// 所以这里把它做成公开常量，`crates/all/tests/framework_jobs.rs` 断言两边一致。
pub const JOB_TARGET_LOGS_RETENTION: &str = "cicada:logs:retention";

/// 全部框架自带任务目标（seed 与组装根的一致性检查用）。
pub const FRAMEWORK_JOB_TARGETS: &[&str] = &[JOB_TARGET_LOGS_RETENTION];

async fn seed_jobs(db: &DatabaseConnection) -> Result<(), DbErr> {
    const COLS: &[&str] = &[
        "job_id",
        "tenant_id",
        "job_name",
        "job_group",
        "invoke_target",
        "cron_expression",
        "concurrency",
        "timeout_secs",
        "max_retry",
        "status",
        "remark",
        "create_by",
        "create_time",
    ];
    insert_row(
        db,
        "sys_job",
        "job_id",
        COLS,
        vec![
            v(1i64),
            v(1i64),
            v("日志分区维护"),
            v("SYSTEM"),
            v(JOB_TARGET_LOGS_RETENTION),
            v("0 0 3 1 * *"),
            v(1i16),
            v(0i64),
            v(0i32),
            v(0i16),
            v("每月 1 日 03:00：扩展日志表分区窗口，归档并删除超过保留期的整月分区（审计日志分区维护）"),
            v("admin"),
            v(chrono::Utc::now()),
        ],
    )
    .await
}
