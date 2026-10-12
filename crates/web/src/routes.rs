//! 路由声明：把 `#[require_perm]` 的元数据收进 `Routes`，供装配期对账与运行时守卫
//! 。
//!
//! 模块的 routes 函数返回 `Routes` 而不是裸 `Router`：裸 `Router` 是不透明树，
//! 框架拿不到逐路由的权限声明，无法在装配期校验「路由声明的权限点是否已注册」。

use axum::Router;
use axum::extract::{Extension, Request};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use rust_i18n::t;

use cicada_audit::AuditMeta;
use cicada_kernel::{Permission, RequestContext};

use crate::audit::audit_middleware;
use crate::problem::{BizCode, Problem};
use crate::state::AppState;

/// 单条路由的元数据：路径 + 声明的权限点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteMeta {
    pub path: String,
    pub perms: Vec<Permission>,
}

/// 挂在方法路由上的权限声明，供守卫中间件读取。
#[derive(Clone)]
pub struct RoutePerms(pub Vec<Permission>);

/// 挂在方法路由上的审计元数据，供审计中间件读取（`#[audit]` 产出的 `AuditMeta`）。
#[derive(Clone)]
pub struct RouteAudit(pub AuditMeta);

/// 路由声明器：包装 `Router` 的同时收集元数据并挂运行时权限守卫。
pub struct Routes {
    router: Router<AppState>,
    metas: Vec<RouteMeta>,
}

impl Default for Routes {
    fn default() -> Self {
        Self::new()
    }
}

impl Routes {
    pub fn new() -> Self {
        Self {
            router: Router::new(),
            metas: Vec::new(),
        }
    }

    /// 声明一条路由。`perms` 非空时挂运行时守卫：任一权限点满足即放行
    /// （与参考实现默认 `Logical.OR` 一致），超级管理员短路。
    pub fn route(
        self,
        path: &str,
        method_router: MethodRouter<AppState>,
        perms: &'static [&'static str],
    ) -> Self {
        let perms = to_perms(perms);
        let method_router = apply_perm_guard(method_router, &perms);
        self.push_route(path, method_router, perms)
    }

    /// 声明一条审计路由：在权限守卫之外再挂审计中间件，请求结束后写 `sys_oper_log`。
    /// `audit` 来自 `#[audit(...)]` 生成的 `<handler>__audit()`。
    pub fn route_audited(
        self,
        path: &str,
        method_router: MethodRouter<AppState>,
        perms: &'static [&'static str],
        audit: AuditMeta,
    ) -> Self {
        let perms = to_perms(perms);
        let method_router = apply_perm_guard(method_router, &perms)
            .route_layer(middleware::from_fn(audit_middleware))
            .layer(Extension(RouteAudit(audit)));
        self.push_route(path, method_router, perms)
    }

    fn push_route(
        mut self,
        path: &str,
        method_router: MethodRouter<AppState>,
        perms: Vec<Permission>,
    ) -> Self {
        self.metas.push(RouteMeta {
            path: path.to_string(),
            perms,
        });
        self.router = self.router.route(path, method_router);
        self
    }

    /// 合并另一份声明（元数据一并合并）。
    pub fn merge(mut self, other: Routes) -> Self {
        self.router = self.router.merge(other.router);
        self.metas.extend(other.metas);
        self
    }

    /// 合并不带元数据的裸路由（如 Swagger UI）。
    pub fn merge_router(mut self, router: Router<AppState>) -> Self {
        self.router = self.router.merge(router);
        self
    }

    pub fn into_parts(self) -> (Router<AppState>, Vec<RouteMeta>) {
        (self.router, self.metas)
    }

    /// 本声明里出现过的全部权限点（去重前的展平）。
    pub fn declared_perms(&self) -> Vec<Permission> {
        self.metas
            .iter()
            .flat_map(|m| m.perms.iter().cloned())
            .collect()
    }
}

fn to_perms(perms: &'static [&'static str]) -> Vec<Permission> {
    perms.iter().map(|p| Permission::new(*p)).collect()
}

fn apply_perm_guard(
    method_router: MethodRouter<AppState>,
    perms: &[Permission],
) -> MethodRouter<AppState> {
    if perms.is_empty() {
        method_router
    } else {
        method_router
            .route_layer(middleware::from_fn(perm_guard))
            .layer(Extension(RoutePerms(perms.to_vec())))
    }
}

/// 运行时权限守卫：读不到 `RoutePerms` 直接放行；读不到 `RequestContext` 视为未认证。
pub async fn perm_guard(req: Request, next: Next) -> Response {
    let required = req.extensions().get::<RoutePerms>().map(|p| p.0.clone());
    if let Some(required) = required {
        match req.extensions().get::<RequestContext>().cloned() {
            // 匿名请求：未认证 401。
            Some(ctx) if ctx.principal.is_none() => {
                return Problem::from_context(
                    BizCode::AuthUnauthorized,
                    t!("auth.unauthorized", locale = ctx.locale.as_str()),
                    &ctx,
                )
                .instance(req.uri().path().to_string())
                .into_response();
            }
            // 已认证但缺权限：403。
            Some(ctx) if !required.iter().any(|p| ctx.has_permission(p)) => {
                return Problem::from_context(
                    BizCode::RbacForbidden,
                    t!("auth.forbidden", locale = ctx.locale.as_str()),
                    &ctx,
                )
                .instance(req.uri().path().to_string())
                .into_response();
            }
            Some(_) => {}
            None => {
                let locale = crate::i18n::negotiate_headers(req.headers());
                return Problem::from_status(
                    StatusCode::UNAUTHORIZED,
                    t!("auth.unauthorized", locale = locale.as_str()),
                )
                .instance(req.uri().path().to_string())
                .into_response();
            }
        }
    }
    next.run(req).await
}
