<script setup lang="ts">
/**
 * 概览页：登录后的默认落点，也是「菜单没授权」时唯一还能看到内容的页面。
 *
 * 刻意只放**解释性信息**（我在哪个租户、什么角色、能做什么、接口打到哪里），不做业务
 * 图表：这一版没有指标数据源，硬造几个假数字比留白更糟。它的真实职责是让用户与排障者
 * 一眼确认「当前会话的身份与范围对不对」。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import AppPage from '@/components/AppPage.vue';
import { useAuthStore } from '@/stores/auth';
import { usePermissionStore } from '@/stores/permission';
import { currentLocale } from '@/locales';

const { t } = useI18n();
const auth = useAuthStore();
const permission = usePermissionStore();

const greeting = computed(() => {
  const hour = new Date().getHours();
  if (hour < 12) return t('dashboard.greetingMorning');
  if (hour < 18) return t('dashboard.greetingAfternoon');
  return t('dashboard.greetingEvening');
});

const displayName = computed(() => auth.userName || t('common.empty'));

/** 接口地址：留空表示同源（生产由 Nginx 反代），这里显示成人能读的说明。 */
const apiBase = computed(() => import.meta.env.VITE_API_BASE_URL || t('dashboard.sameOrigin'));

const facts = computed(() => [
  { key: 'dashboard.account', value: displayName.value },
  { key: 'dashboard.tenant', value: auth.tenantId === null ? '—' : String(auth.tenantId) },
  {
    key: 'dashboard.roles',
    value: auth.roles.length > 0 ? auth.roles.join('、') : t('dashboard.noRole'),
  },
  { key: 'dashboard.permissions', value: String(auth.permissions.length) },
  { key: 'dashboard.menus', value: String(permission.menuCount) },
]);

const environment = computed(() => [
  { key: 'dashboard.apiBase', value: apiBase.value },
  { key: 'layout.language', value: currentLocale() },
  { key: 'dashboard.version', value: __APP_VERSION__ },
]);
</script>

<template>
  <AppPage>
    <div class="dashboard__hero">
      <p class="dashboard__greeting">{{ greeting }}</p>
      <h1 class="dashboard__welcome">
        {{ t('dashboard.welcome', { name: displayName }) }}
      </h1>
      <p class="dashboard__hint">{{ t('dashboard.welcomeHint') }}</p>
    </div>

    <div class="dashboard__grid">
      <section class="dashboard__card ci-card">
        <h2 class="dashboard__card-title">{{ t('dashboard.session') }}</h2>
        <dl class="dashboard__facts">
          <template v-for="fact in facts" :key="fact.key">
            <dt>{{ t(fact.key) }}</dt>
            <dd>{{ fact.value }}</dd>
          </template>
        </dl>
      </section>

      <section class="dashboard__card ci-card">
        <h2 class="dashboard__card-title">{{ t('dashboard.environment') }}</h2>
        <dl class="dashboard__facts">
          <template v-for="fact in environment" :key="fact.key">
            <dt>{{ t(fact.key) }}</dt>
            <dd>{{ fact.value }}</dd>
          </template>
        </dl>
      </section>

      <section class="dashboard__card ci-card dashboard__card--wide">
        <h2 class="dashboard__card-title">{{ t('dashboard.quickStart') }}</h2>
        <p class="dashboard__hint">{{ t('dashboard.quickStartHint') }}</p>
        <div class="dashboard__tags">
          <el-tag v-for="perm in auth.permissions.slice(0, 12)" :key="perm" effect="plain" round>
            {{ perm }}
          </el-tag>
          <span v-if="auth.permissions.length > 12" class="ci-tertiary">
            +{{ auth.permissions.length - 12 }}
          </span>
        </div>
      </section>
    </div>
  </AppPage>
</template>

<style scoped>
.dashboard__hero {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-1);
  padding: var(--ci-space-6);
  border-radius: var(--ci-radius-xl);
  background: var(--ci-color-surface);
  border: 1px solid var(--ci-color-separator);
  box-shadow: var(--ci-shadow-1);
}

.dashboard__greeting {
  font-size: var(--ci-text-sm);
  color: var(--ci-color-text-tertiary);
}

.dashboard__welcome {
  font-size: var(--ci-text-2xl);
  font-weight: var(--ci-weight-semibold);
  letter-spacing: var(--ci-tracking-2xl);
  color: var(--ci-color-text);
}

.dashboard__hint {
  max-width: 68ch;
  font-size: var(--ci-text-sm);
  line-height: var(--ci-leading-relaxed);
  color: var(--ci-color-text-secondary);
}

.dashboard__grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
  gap: var(--ci-space-4);
}

.dashboard__card {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  padding: var(--ci-space-5);
}

.dashboard__card--wide {
  grid-column: 1 / -1;
}

.dashboard__card-title {
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.dashboard__facts {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: var(--ci-space-2) var(--ci-space-4);
  font-size: var(--ci-text-sm);
}

.dashboard__facts dt {
  color: var(--ci-color-text-tertiary);
}

.dashboard__facts dd {
  margin: 0;
  color: var(--ci-color-text);
  overflow-wrap: anywhere;
}

.dashboard__tags {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--ci-space-2);
  font-size: var(--ci-text-xs);
}
</style>
