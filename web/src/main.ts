/**
 * 应用入口。
 *
 * 样式顺序有意义：Element Plus 的原始皮肤先加载，我们的令牌化主题（`styles/index.css`）
 * 后加载并接管 `--el-*` 变量，因此不需要 fork 组件库的样式。
 *
 * 这里也是「会话失效」的唯一接线处：拦截器只负责清会话，跳转与提示在应用层面决定——
 * 拦截器里拿不到路由与 store，硬跳 `location.href` 会丢掉 SPA 状态。
 */
import ElementPlus from 'element-plus';
import { createPinia } from 'pinia';
import { createApp } from 'vue';

import 'element-plus/dist/index.css';
import '@/styles/index.css';

import App from '@/App.vue';
import { installPermDirectives } from '@/directives/perm';
import { i18n, translate } from '@/locales';
import { router } from '@/router';
import { LOGIN_PATH } from '@/router/guard';
import { useAuthStore } from '@/stores/auth';
import { usePermissionStore } from '@/stores/permission';
import { notifyWarning } from '@/utils/feedback';
import { onSessionCleared } from '@/utils/session';

const app = createApp(App);
const pinia = createPinia();

app.use(pinia);
app.use(i18n);
app.use(router);
app.use(ElementPlus);

// `v-perm` / `v-hasPermi` / `v-hasRole`：前端侧的入口显隐。
// 它只是界面便利，真正的判定永远在后端。
installPermDirectives(app);

const auth = useAuthStore(pinia);
const permission = usePermissionStore(pinia);

/**
 * 会话被动终止（刷新令牌过期、被管理员强退、改密）时的统一收尾。
 *
 * 触发点只有一个：`clearSession('expired')` 的广播（`utils/session.ts`）。用户主动登出
 * 走的是 `logout`，不广播——那种情况由 `AppHeader` 自己跳登录页，不该弹「登录已过期」。
 *
 * 注意这里用 `forget()` 而不是 `auth.reset()`：令牌早在广播之前就清掉了，`reset()` 会
 * 再清一次并重新广播回来，形成回调递归（真实踩过 `RangeError: Maximum call stack`）。
 */
function handleSessionLost(): void {
  permission.reset();
  auth.forget();
  const current = router.currentRoute.value;
  if (current.path === LOGIN_PATH) return;
  notifyWarning(translate('common.sessionExpired'));
  void router.replace({
    path: LOGIN_PATH,
    query: current.fullPath === '/' ? {} : { redirect: current.fullPath },
  });
}

onSessionCleared(handleSessionLost);

// 首屏先恢复主题/语言（store 在构造时就已 apply），再挂载，避免闪一下默认外观。
app.mount('#app');
