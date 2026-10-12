import { readFileSync } from 'node:fs';
import { fileURLToPath, URL } from 'node:url';

import vue from '@vitejs/plugin-vue';
import { defineConfig, loadEnv } from 'vite';

/**
 * 前端独立构建：产物交给 Nginx/CDN 静态托管，不由后端内嵌。
 *
 * 开发期用代理而不是 CORS：前端始终按同源相对路径发请求（`/api/...`、`/auth/...`），
 * 开发时由本配置转发到后端，生产由 Nginx 反代——两边行为一致，也省掉一份跨域白名单。
 */
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), 'VITE_');
  const proxyTarget = env.VITE_DEV_PROXY_TARGET || 'http://127.0.0.1:8081';
  // 版本号从 package.json 读，概览页要显示「跑的是哪一版前端」（排障时最常被问的一句）。
  const { version } = JSON.parse(
    readFileSync(new URL('./package.json', import.meta.url), 'utf-8'),
  ) as { version: string };

  return {
    plugins: [vue()],
    define: { __APP_VERSION__: JSON.stringify(version) },
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
      },
    },
    server: {
      port: Number(env.VITE_DEV_PORT || 5173),
      strictPort: false,
      proxy: {
        '/api': { target: proxyTarget, changeOrigin: true },
        '/auth': { target: proxyTarget, changeOrigin: true },
      },
    },
    build: {
      target: 'es2022',
      sourcemap: false,
      // Element Plus 全量注册（生成器产物直接用 `el-*` 标签，不做按需扫描），
      // 单文件体积因此偏高，这里只把警戒线抬高，不做静默忽略。
      chunkSizeWarningLimit: 1500,
    },
  };
});
