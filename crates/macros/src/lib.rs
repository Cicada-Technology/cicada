//! cicada-macros（过程宏）
//!
//! - `#[derive(DataScoped)]`：实体声明归属维度（datascope-typestate.md 项目目标）；
//! - `#[require_perm("...")]`：handler 权限点声明，编译期写入路由元数据；
//! - `#[audit(...)]`：handler 审计元数据声明，编译期产出 `<name>__audit()`。

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{DeriveInput, ItemFn, LitStr, Path, Token, parse_macro_input};

/// 声明 handler 的审计元数据，编译期产出 `<name>__audit()` 伴随函数。
///
/// ```ignore
/// #[cicada_macros::audit(module = "system", action = "user:add", kind = "insert")]
/// async fn create_user() -> &'static str { "ok" }
///
/// let meta = create_user__audit();
/// assert_eq!(meta.title(), "system:user:add");
/// assert_eq!(meta.business_type, 1);
/// ```
///
/// `kind` 取 `other|insert|update|delete|grant|export|import|force|gencode|clean`
/// （映射参考实现 `BusinessType` 数值），缺省 `other`。注册路由时把
/// `<name>__audit()` 交给 `Routes::route_audited(path, method, perms, audit)`，
/// 由 `cicada-web` 的 `audit_middleware` 在请求结束后写 `sys_oper_log`。
#[proc_macro_attribute]
pub fn audit(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = TokenStream2::from(attr);
    let func = parse_macro_input!(item as ItemFn);
    expand_audit(attr, func)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_audit(attr: TokenStream2, func: ItemFn) -> syn::Result<TokenStream2> {
    let mut module: Option<LitStr> = None;
    let mut action: Option<LitStr> = None;
    let mut kind: Option<LitStr> = None;

    let meta_parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("module") {
            module = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("action") {
            action = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("kind") {
            kind = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("未知的 audit 选项（可用 module / action / kind）"));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(meta_parser, attr)?;

    let module = module
        .ok_or_else(|| syn::Error::new_spanned(&func.sig.ident, "#[audit] 缺少 module = \"..\""))?;
    let action = action
        .ok_or_else(|| syn::Error::new_spanned(&func.sig.ident, "#[audit] 缺少 action = \"..\""))?;

    let business_type = match &kind {
        Some(k) => match k.value().as_str() {
            "other" => 0i16,
            "insert" => 1,
            "update" => 2,
            "delete" => 3,
            "grant" => 4,
            "export" => 5,
            "import" => 6,
            "force" => 7,
            "gencode" | "gen_code" => 8,
            "clean" => 9,
            other => {
                return Err(syn::Error::new_spanned(
                    k,
                    format!(
                        "未知的 kind \"{other}\"（可用 other|insert|update|delete|grant|export|import|force|gencode|clean）"
                    ),
                ));
            }
        },
        None => 0,
    };

    let ident = &func.sig.ident;
    let vis = &func.vis;
    let meta_ident = syn::Ident::new(&format!("{ident}__audit"), ident.span());

    Ok(quote! {
        #func

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #meta_ident() -> ::cicada_audit::AuditMeta {
            ::cicada_audit::AuditMeta {
                module: #module,
                action: #action,
                business_type: #business_type,
            }
        }
    })
}

/// 声明 handler 需要的权限点，编译期把标识写进路由元数据。
///
/// 宏为同名 handler 生成一个 `<name>__perms()` 伴随函数，返回权限标识切片；
/// 注册路由时把它交给 `Routes::route(path, method_router, <name>__perms())`，
/// 由 `cicada-web` 在装配期与 `Manifest.perms` 双向对账，并挂运行时守卫。
#[proc_macro_attribute]
pub fn require_perm(attr: TokenStream, item: TokenStream) -> TokenStream {
    let perms = parse_macro_input!(attr with Punctuated::<LitStr, Token![,]>::parse_terminated);
    let func = parse_macro_input!(item as ItemFn);
    expand_require_perm(perms, func)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_require_perm(
    perms: Punctuated<LitStr, Token![,]>,
    func: ItemFn,
) -> syn::Result<TokenStream2> {
    if perms.is_empty() {
        return Err(syn::Error::new_spanned(
            &func.sig.ident,
            "#[require_perm] 至少需要一个权限标识，如 #[require_perm(\"system:user:add\")]",
        ));
    }
    let ident = &func.sig.ident;
    let vis = &func.vis;
    let meta_ident = syn::Ident::new(&format!("{ident}__perms"), ident.span());
    let lits: Vec<&LitStr> = perms.iter().collect();

    Ok(quote! {
        #func

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #meta_ident() -> &'static [&'static str] {
            &[ #(#lits),* ]
        }
    })
}

/// 实体归属维度声明（唯一实现 `cicada_kernel::scope::DataScoped`）。
///
/// 用法：
/// ```ignore
/// #[derive(DeriveEntityModel, DataScoped)]
/// #[sea_orm(table_name = "biz_product")]
/// #[data_scope(tenant = Column::TenantId, dept = Column::DeptId,
///              owner = Column::CreateBy, deleted = Column::DeletedAt)]
/// pub struct Model { /* ... */ }
/// ```
///
/// 列形态只能四选一：`dept + owner`（DeptAndOwner）、仅 `dept`（DeptOnly）、
/// 仅 `owner`（OwnerOnly）、两者都无（必须显式 `no_ownership` 或 `exempt(reason = "...")`）。
/// 什么都不写是编译错误——默认路径是拒绝，不是放行。
#[proc_macro_derive(DataScoped, attributes(data_scope))]
pub fn derive_data_scope(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_data_scope(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_data_scope(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let attr = input
        .attrs
        .iter()
        .find(|a| a.path().is_ident("data_scope"))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                input,
                "缺少 #[data_scope(tenant = .., dept = .. | owner = .. | no_ownership | exempt(reason = \"..\"))] 声明",
            )
        })?;

    let mut tenant: Option<Path> = None;
    let mut dept: Option<Path> = None;
    let mut owner: Option<Path> = None;
    let mut deleted: Option<Path> = None;
    let mut no_ownership = false;
    let mut exempt = false;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("tenant") {
            tenant = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("dept") {
            dept = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("owner") {
            owner = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("deleted") {
            deleted = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("no_ownership") {
            no_ownership = true;
        } else if meta.path.is_ident("exempt") {
            exempt = true;
            meta.parse_nested_meta(|nested| {
                if nested.path.is_ident("reason") {
                    let _reason: LitStr = nested.value()?.parse()?;
                }
                Ok(())
            })?;
        } else {
            return Err(meta.error("未知的 data_scope 选项"));
        }
        Ok(())
    })?;

    let tenant =
        tenant.ok_or_else(|| syn::Error::new_spanned(attr, "data_scope 必须声明 tenant 列"))?;

    let has_column = dept.is_some() || owner.is_some();
    if has_column && (no_ownership || exempt) {
        return Err(syn::Error::new_spanned(
            attr,
            "dept/owner 列与 no_ownership/exempt 不能同时出现",
        ));
    }
    if no_ownership && exempt {
        return Err(syn::Error::new_spanned(
            attr,
            "no_ownership 与 exempt 不能同时出现",
        ));
    }

    let shape = match (dept.is_some(), owner.is_some()) {
        (true, true) => quote!(cicada_kernel::scope::DeptAndOwner),
        (true, false) => quote!(cicada_kernel::scope::DeptOnly),
        (false, true) => quote!(cicada_kernel::scope::OwnerOnly),
        (false, false) if no_ownership => quote!(cicada_kernel::scope::NoOwnership),
        (false, false) if exempt => quote!(cicada_kernel::scope::Exempt),
        (false, false) => {
            return Err(syn::Error::new_spanned(
                attr,
                "实体必须声明归属维度：给出 dept/owner 列，或显式 no_ownership / exempt(reason = \"...\")",
            ));
        }
    };

    let dept_column = option_expr(dept);
    let owner_column = option_expr(owner);
    let deleted_column = option_expr(deleted);

    Ok(quote! {
        impl cicada_kernel::scope::DataScoped for Entity {
            type Shape = #shape;

            fn tenant_column() -> Self::Column {
                #tenant
            }

            fn soft_delete_column() -> Option<Self::Column> {
                #deleted_column
            }

            fn dept_column() -> Option<Self::Column> {
                #dept_column
            }

            fn owner_column() -> Option<Self::Column> {
                #owner_column
            }
        }
    })
}

fn option_expr(path: Option<Path>) -> TokenStream2 {
    match path {
        Some(p) => quote!(Some(#p)),
        None => quote!(None),
    }
}

/// 字段级数据脱敏。
///
/// 用法：
/// ```ignore
/// #[derive(Serialize, Maskable)]
/// pub struct UserVo {
///     #[mask(Mobile)]
///     pub phonenumber: Option<String>,
///     #[mask(Email)]
///     pub email: String,
/// }
///
/// let vo = vo.masked(policy); // policy: MaskPolicy::{Plain, Masked}
/// ```
///
/// `#[mask(..)]` 取值 `Mobile | IdCard | Email | Name`；字段类型支持
/// `String` 与 `Option<String>`，其余类型是编译错误。派生只生成
/// `cicada_kernel::mask::Maskable` 实现，何时掩码由调用方（handler 边界）
/// 依据 `MaskPolicy` 决定——掩码永远发生在出参侧，不影响入库值。
#[proc_macro_derive(Maskable, attributes(mask))]
pub fn derive_maskable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_maskable(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_maskable(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        syn::Data::Struct(s) => &s.fields,
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "Maskable 目前只支持结构体（字段级 #[mask]）",
            ));
        }
    };

    let mut stmts = Vec::new();
    for f in fields.iter() {
        let Some(attr) = f.attrs.iter().find(|a| a.path().is_ident("mask")) else {
            continue;
        };
        let ident = f
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(attr, "#[mask] 只能用于具名字段"))?;
        let kind = attr.parse_args::<Path>()?;
        let kind_ident = kind
            .get_ident()
            .ok_or_else(|| {
                syn::Error::new_spanned(&kind, "#[mask] 取值应为 Mobile|IdCard|Email|Name")
            })?
            .clone();
        let kind_str = kind_ident.to_string();
        let kind = match kind_str.as_str() {
            "Mobile" | "IdCard" | "Email" | "Name" => {
                let k = syn::Ident::new(&kind_str, kind_ident.span());
                quote!(cicada_kernel::mask::MaskKind::#k)
            }
            other => {
                return Err(syn::Error::new_spanned(
                    &kind_ident,
                    format!("未知的 mask 类型 \"{other}\"（可用 Mobile|IdCard|Email|Name）"),
                ));
            }
        };

        let ty = f.ty.clone();
        // 判定 String / Option<String>（允许路径前缀）。
        let is_string = |ty: &syn::Type| {
            if let syn::Type::Path(p) = ty {
                let seg = p.path.segments.last();
                return seg.is_some_and(|s| s.ident == "String");
            }
            false
        };
        let mut option_inner_string = false;
        if let syn::Type::Path(p) = &ty
            && let Some(seg) = p.path.segments.last()
            && seg.ident == "Option"
            && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
            && args.args.len() == 1
            && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
        {
            option_inner_string = is_string(inner);
        }

        let stmt = if is_string(&ty) {
            quote! {
                self.#ident = cicada_kernel::mask::mask(#kind, &self.#ident);
            }
        } else if option_inner_string {
            quote! {
                if let Some(v) = self.#ident.as_ref() {
                    self.#ident = Some(cicada_kernel::mask::mask(#kind, v));
                }
            }
        } else {
            return Err(syn::Error::new_spanned(
                ty,
                "#[mask] 字段类型目前只支持 String 与 Option<String>",
            ));
        };
        stmts.push(stmt);
    }

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics cicada_kernel::mask::Maskable for #name #ty_generics #where_clause {
            fn apply_mask(&mut self, policy: cicada_kernel::mask::MaskPolicy) {
                if policy.is_masked() {
                    #( #stmts )*
                }
            }
        }
    })
}
