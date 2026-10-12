/**
 * 前端权限指令（权限判断各端自实现，v1 用 `v-hasPermi` / `v-hasRole`）。
 *
 * 三种写法都支持，语义一致——`v-perm` 是代码生成器产出页面用的名字，
 * `v-hasPermi` / `v-hasRole` 是参考实现迁移过来的习惯写法：
 *
 * ```html
 * <el-button v-perm="'system:user:add'">新增</el-button>
 * <el-button v-hasPermi="['system:user:edit', 'system:user:add']">任一即可</el-button>
 * <el-button v-hasRole="['admin']">仅管理员可见</el-button>
 * <el-button v-perm.disable="'system:user:remove'">无权限时置灰而不是隐藏</el-button>
 * ```
 *
 * 这是**界面便利**，不是安全边界：真正的判定在后端的 `#[require_perm]` 守卫。
 * 因此这里用内联 `display: none` 而不是移除节点——可逆、可重算（权限集合刷新后无需重建组件）。
 */

import type { App, Directive, DirectiveBinding } from 'vue';

import { useAuthStore } from '@/stores/auth';
import { translate } from '@/locales';

export type PermValue = string | string[] | undefined;

type CheckKind = 'perm' | 'role';

function normalize(value: PermValue): string[] {
  if (!value) return [];
  return (Array.isArray(value) ? value : [value]).filter((item) => item !== '');
}

function allows(kind: CheckKind, value: PermValue): boolean {
  const keys = normalize(value);
  // 空值视为「无约束」：漏写参数不应该把所有按钮都藏起来。
  if (keys.length === 0) return true;
  const auth = useAuthStore();
  return keys.some((key) => (kind === 'perm' ? auth.hasPermission(key) : auth.hasRole(key)));
}

/** 记住原始内联 display，恢复时才不会把 `inline-flex` 之类写死。 */
const ORIGINAL_DISPLAY = new WeakMap<HTMLElement, string>();

function setVisibility(el: HTMLElement, visible: boolean): void {
  if (!ORIGINAL_DISPLAY.has(el)) {
    ORIGINAL_DISPLAY.set(el, el.style.display);
  }
  el.style.display = visible ? (ORIGINAL_DISPLAY.get(el) ?? '') : 'none';
}

function setDisabled(el: HTMLElement, disabled: boolean): void {
  el.classList.toggle('ci-perm-denied', disabled);
  el.toggleAttribute('disabled', disabled);
  el.setAttribute('aria-disabled', String(disabled));
  if (disabled) {
    el.setAttribute('title', translate('common.forbidden'));
  } else {
    el.removeAttribute('title');
  }
  el.style.cursor = disabled ? 'not-allowed' : '';
  el.style.opacity = disabled ? '0.5' : '';
}

function apply(el: HTMLElement, binding: DirectiveBinding<PermValue>, kind: CheckKind): void {
  const allowed = allows(kind, binding.value);
  if (binding.modifiers.disable) {
    setDisabled(el, !allowed);
    return;
  }
  setVisibility(el, allowed);
}

function makeDirective(kind: CheckKind): Directive<HTMLElement, PermValue> {
  return {
    mounted: (el, binding) => apply(el, binding, kind),
    updated: (el, binding) => apply(el, binding, kind),
  };
}

export const permDirectives: Record<string, Directive<HTMLElement, PermValue>> = {
  perm: makeDirective('perm'),
  hasPermi: makeDirective('perm'),
  hasRole: makeDirective('role'),
};

export function installPermDirectives(app: App): void {
  for (const [name, directive] of Object.entries(permDirectives)) {
    app.directive(name, directive);
  }
}
