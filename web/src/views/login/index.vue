<script setup lang="ts">
/**
 * 登录页。
 *
 * 三件事必须按顺序做，顺序错了会出现「登录成功但侧栏是上一个账号的菜单」：
 * 1. `signIn` 换令牌；
 * 2. `loadProfile` 拉权限集合（守卫会用它做 `meta.perms` 校验）；
 * 3. `permission.reset()` 撤掉旧账号注册的动态路由，随后由守卫重新装载。
 *
 * 失败提示走**内联**而不是全局 toast：账号不存在 / 密码错 / 已锁定 / 需要动态码
 * 是四条不同的处理路径，用户需要看着表单修正，弹窗飘走就看不清了。
 */
import type { FormInstance, FormItemRule } from 'element-plus';
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute, useRouter } from 'vue-router';

import { getCaptcha } from '@/api/auth';
import { HOME_PATH } from '@/router/routes';
import { useAppStore } from '@/stores/app';
import { useAuthStore } from '@/stores/auth';
import { usePermissionStore } from '@/stores/permission';
import { ApiError } from '@/utils/api-error';

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const app = useAppStore();
const auth = useAuthStore();
const permission = usePermissionStore();
/** 横版标志按主题切换；深色卡片上用白色字标。 */
const brandLogo = computed(() => (app.isDark ? '/brand/logo-dark.svg' : '/brand/logo.svg'));

const formRef = ref<FormInstance>();
const loading = ref(false);
const errorMessage = ref('');

const form = reactive({ username: '', password: '', totp: '', captchaCode: '' });

/** 图形验证码：后端在连续失败后返回「图形验证码」错误，届时才展示。 */
const captchaNeeded = ref(false);
const captcha = reactive({ id: '', img: '' });

async function refreshCaptcha(): Promise<void> {
  try {
    const vo = await getCaptcha();
    captchaNeeded.value = vo.enabled;
    captcha.id = vo.captcha_id;
    captcha.img = vo.img;
  } catch {
    /* 验证码服务故障不阻塞表单，提交时后端仍会兜底闸门 */
  }
}

/** 模板表达式里不能用 `import.meta`（编译器按普通 JS 解析），所以在这里取一次。 */
const isDev = import.meta.env.DEV;

const rules: Record<string, FormItemRule[]> = {
  username: [{ required: true, message: t('login.emptyUsername'), trigger: 'blur' }],
  password: [{ required: true, message: t('login.emptyPassword'), trigger: 'blur' }],
};

/** 登录后的落点：地址栏里的 redirect 优先，其次第一个可见菜单，最后概览。 */
const redirectTarget = computed(() => {
  const raw = route.query.redirect;
  const target = typeof raw === 'string' ? raw : '';
  // 只接受站内绝对路径：`//evil.com` 这类协议相对地址会被浏览器当成外域。
  if (target.startsWith('/') && !target.startsWith('//')) return target;
  return null;
});

async function submit(): Promise<void> {
  errorMessage.value = '';
  const valid = await formRef.value?.validate().catch(() => false);
  if (!valid) return;

  loading.value = true;
  try {
    await auth.signIn({
      username: form.username.trim(),
      password: form.password,
      totp: form.totp.trim() === '' ? undefined : form.totp.trim(),
      captcha_id: captchaNeeded.value ? captcha.id : undefined,
      captcha_code: captchaNeeded.value ? form.captchaCode.trim() || undefined : undefined,
    });
    await auth.loadProfile();
    // 动态路由属于上一个账号：先撤干净，交给守卫按新权限重建。
    permission.reset();
    await router.replace(redirectTarget.value ?? HOME_PATH);
  } catch (error) {
    errorMessage.value = error instanceof ApiError ? error.message : t('login.failed');
    // 后端要求验证码（首次触发或答案错/过期）：展示并取一张新的。
    if (errorMessage.value.includes('图形验证码')) {
      captchaNeeded.value = true;
    }
    // 验证码是一次性的：任何失败都可能已被后端消费，统一换一张。
    if (captchaNeeded.value) {
      form.captchaCode = '';
      await refreshCaptcha();
    }
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="login">
    <!--
      卡片用 div 而不是 form：`el-form` 自己渲染一个 `<form>`，两者嵌套时浏览器/框架对
      submit 事件的处理并不等价（实测点提交按钮不会触达外层处理器）。提交挂在 el-form 上，
      「回车提交」由它的 submit 事件天然覆盖。
    -->
    <div class="login__card ci-material">
      <header class="login__head">
        <img class="login__brand" :src="brandLogo" :alt="t('app.brand')" draggable="false" />
        <h1 class="login__title">{{ t('app.title') }}</h1>
        <p class="login__subtitle">{{ t('login.subtitle') }}</p>
      </header>

      <el-form
        ref="formRef"
        :model="form"
        :rules="rules"
        label-position="top"
        class="login__form"
        @submit.prevent="submit"
      >
        <el-form-item :label="t('login.username')" prop="username">
          <el-input
            v-model="form.username"
            size="large"
            autocomplete="username"
            :placeholder="t('login.usernamePlaceholder')"
          />
        </el-form-item>

        <el-form-item :label="t('login.password')" prop="password">
          <el-input
            v-model="form.password"
            type="password"
            size="large"
            show-password
            autocomplete="current-password"
            :placeholder="t('login.passwordPlaceholder')"
          />
        </el-form-item>

        <el-form-item :label="t('login.totp')" prop="totp">
          <el-input
            v-model="form.totp"
            size="large"
            maxlength="6"
            autocomplete="one-time-code"
            :placeholder="t('login.totpPlaceholder')"
          />
        </el-form-item>

        <el-form-item v-if="captchaNeeded" :label="t('login.captcha')" prop="captchaCode">
          <div class="login__captcha">
            <el-input
              v-model="form.captchaCode"
              size="large"
              maxlength="6"
              :placeholder="t('login.captchaPlaceholder')"
            />
            <img
              v-if="captcha.img"
              class="login__captcha-img"
              :src="`data:image/png;base64,${captcha.img}`"
              :alt="t('login.captcha')"
              @click="refreshCaptcha"
            />
          </div>
        </el-form-item>

        <p v-if="errorMessage" class="login__error" role="alert">{{ errorMessage }}</p>

        <el-button
          type="primary"
          size="large"
          native-type="submit"
          class="login__submit"
          :loading="loading"
        >
          {{ loading ? t('login.submitting') : t('login.submit') }}
        </el-button>
      </el-form>

      <!-- 开发期提示种子管理员账号（`migrations/src/seed.rs`）：生产构建里这段不存在。 -->
      <p v-if="isDev" class="login__dev-hint">
        开发模式：种子管理员 <code>admin / Cicada@2026</code>
      </p>
    </div>
  </div>
</template>

<style scoped>
.login {
  display: grid;
  place-items: center;
  min-height: 100vh;
  padding: var(--ci-space-4);
  /* 背景给一层极淡的径向光：登录页是唯一没有内容的页面，纯平背景下卡片会显得悬浮无依。 */
  background:
    radial-gradient(120% 80% at 50% 0%, var(--ci-color-accent-soft), transparent 60%),
    var(--ci-color-bg);
}

.login__card {
  width: 100%;
  max-width: 400px;
  padding: var(--ci-space-8);
  border-radius: var(--ci-radius-2xl);
}

.login__head {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--ci-space-2);
  margin-bottom: var(--ci-space-6);
  text-align: center;
}

.login__brand {
  display: block;
  width: 168px;
  height: auto;
}

.login__title {
  font-size: var(--ci-text-xl);
  font-weight: var(--ci-weight-semibold);
  letter-spacing: var(--ci-tracking-xl);
  color: var(--ci-color-text);
}

.login__subtitle {
  font-size: var(--ci-text-sm);
  color: var(--ci-color-text-secondary);
}

.login__form {
  display: flex;
  flex-direction: column;
}

.login__error {
  margin-bottom: var(--ci-space-3);
  padding: var(--ci-space-2) var(--ci-space-3);
  border-radius: var(--ci-radius-sm);
  background: var(--ci-color-danger-soft);
  color: var(--ci-color-danger);
  font-size: var(--ci-text-sm);
  line-height: var(--ci-leading-snug);
}

.login__submit {
  width: 100%;
}

.login__captcha {
  display: flex;
  gap: var(--ci-space-3);
  width: 100%;
}

.login__captcha-img {
  height: 40px;
  border-radius: var(--ci-radius-sm);
  cursor: pointer;
}

.login__dev-hint {
  margin-top: var(--ci-space-4);
  text-align: center;
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.login__dev-hint code {
  font-family: var(--ci-font-mono);
}
</style>
