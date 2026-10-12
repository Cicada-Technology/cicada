<script setup lang="ts">
/**
 * 外壳：侧栏 + 顶栏 + 路由视图（半透明材料表达层级）。
 *
 * 两个决定：
 * - **侧栏只挂载一次**：动态菜单作为根路由的 children 注册（`router/dynamic.ts`），
 *   切页面时这里不动，所以不会重建菜单、不会丢展开状态。
 * - **窄屏改用抽屉**：侧栏在手机上会挤掉内容，此时它应当是「随时呼出的一层」而不是
 *   常驻栏位；顶栏那个折叠按钮在窄屏即「呼出抽屉」，用户只学一个按钮。
 */
import { computed, ref } from 'vue';

import AppDrawer from '@/components/AppDrawer.vue';
import AppErrorBoundary from '@/components/AppErrorBoundary.vue';
import AppHeader from '@/layout/AppHeader.vue';
import AppSidebar from '@/layout/AppSidebar.vue';
import { useAppStore } from '@/stores/app';

/** 与 `AppHeader` 的窄屏断点保持一致（那里用同一条 `max-width` 收起搜索框）。 */
const MOBILE_QUERY = '(max-width: 900px)';

const app = useAppStore();
const media = window.matchMedia(MOBILE_QUERY);
const isMobile = ref(media.matches);
const drawerOpen = ref(false);

media.addEventListener('change', (event) => {
  isMobile.value = event.matches;
  if (!event.matches) drawerOpen.value = false;
});

const collapsed = computed(() => (isMobile.value ? false : app.sidebarCollapsed));

function toggleSidebar(): void {
  if (isMobile.value) {
    drawerOpen.value = !drawerOpen.value;
  } else {
    app.toggleSidebar();
  }
}
</script>

<template>
  <div class="app-layout" :class="{ 'is-mobile': isMobile }">
    <aside v-if="!isMobile" class="app-layout__sidebar">
      <AppSidebar :collapsed="collapsed" />
    </aside>

    <AppDrawer v-else v-model="drawerOpen" direction="ltr" :size="260">
      <AppSidebar mobile @navigate="drawerOpen = false" />
    </AppDrawer>

    <div class="app-layout__main">
      <AppHeader @toggle-sidebar="toggleSidebar" />

      <main class="app-layout__content">
        <div class="app-layout__container">
          <!-- 页面级边界：某页渲染坏了只废掉内容区，侧栏还能点，用户换页即自救。 -->
          <AppErrorBoundary>
            <RouterView v-slot="{ Component, route }">
              <Transition name="ci-view" mode="out-in">
                <KeepAlive v-if="route.meta.keepAlive">
                  <component :is="Component" :key="route.name ?? route.path" />
                </KeepAlive>
                <component :is="Component" v-else :key="route.name ?? route.path" />
              </Transition>
            </RouterView>
          </AppErrorBoundary>
        </div>
      </main>
    </div>
  </div>
</template>

<style scoped>
.app-layout {
  display: flex;
  height: 100vh;
  min-height: 0;
  background: var(--ci-color-bg);
}

.app-layout__sidebar {
  position: sticky;
  top: 0;
  z-index: var(--ci-z-sidebar);
  height: 100vh;
  flex-shrink: 0;
}

.app-layout__main {
  display: flex;
  flex-direction: column;
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
}

/* 内容区自己滚动：body 锁 `overflow:hidden` 后，长表格/长表单必须在这里获得滚动条。 */
.app-layout__content {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: var(--ci-layout-gutter);
}

/* 宽屏时把内容收在一条阅读宽度上：超宽表格贴边会让人来回扫视整屏。 */
.app-layout__container {
  width: 100%;
  max-width: var(--ci-layout-content-max);
  margin: 0 auto;
}

/* 抽屉模式下侧栏在抽屉里已自带内边距，内容区跟着收一点。 */
.app-layout.is-mobile .app-layout__content {
  padding: var(--ci-space-4);
}
</style>
