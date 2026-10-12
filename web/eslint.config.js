/**
 * ESLint 扁平配置（前端门禁）。
 *
 * 三条取舍：
 * - 只开 `recommended` 档，不引类型感知（type-checked）规则：那需要 tsconfig 的
 *   projectService，lint 会慢一个数量级，而类型错误本来就由 `vue-tsc` 单独把关。
 * - 排版类规则一律交给 Prettier（末尾 `eslint-config-prettier` 关掉全部冲突项），
 *   ESLint 只管「可能是 bug」的规则，两者职责不重叠。
 * - `vue/multi-word-component-names` 关闭：页面文件名固定是 `index.vue`
 *   （generator 产物也是），改名字只为讨好 lint 是本末倒置。
 */
import js from '@eslint/js';
import eslintConfigPrettier from 'eslint-config-prettier';
import pluginVue from 'eslint-plugin-vue';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['dist/**', 'node_modules/**', 'public/**'] },

  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...pluginVue.configs['flat/recommended'],

  {
    files: ['**/*.vue'],
    languageOptions: {
      parserOptions: {
        parser: tseslint.parser,
        ecmaVersion: 'latest',
        sourceType: 'module',
      },
    },
  },

  {
    files: ['**/*.{ts,vue}'],
    languageOptions: {
      globals: { ...globals.browser },
    },
    rules: {
      // TypeScript 自己会报未定义标识符，ESLint 的版本对 TS 语法只会误报。
      'no-undef': 'off',
      // 页面文件名固定为 index.vue、组件目录用 App*：命名靠约定，不靠 lint。
      'vue/multi-word-component-names': 'off',
      // 模板里用到的组件必须来自 import：漏引 `AppTable` 时 Vue 会把它当原生标签渲染，
      // 于是 `<el-table-column>` 找不到所属表格直接抛错，整个应用被打空（真实踩过，
      // 见 `components/AppErrorBoundary.vue` 的注释）。这条规则把这类错误挡在提交前。
      'vue/no-undef-components': [
        'error',
        {
          ignorePatterns: [
            // Element Plus 与 Vue 内置组件来自 `app.use` / 编译器，本就不在源码里 import。
            '^el-',
            '^El[A-Z]',
            '^Router(View|Link)$',
            '^TransitionGroup$',
            '^Transition$',
            '^KeepAlive$',
            '^Teleport$',
            '^Suspense$',
            // 自递归组件（`AppNavList` 在自身模板里引用自己）由文件名提供名字。
            '^AppNavList$',
          ],
        },
      ],
      // 生成的业务代码（`Partial<Vo>`、动态字段）难免出现 any，先降级为 warn。
      '@typescript-eslint/no-explicit-any': 'warn',
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrors: 'none' },
      ],
    },
  },

  {
    // AppForm 的设计就是「字段由数据描述，控件直接写回传入的响应式 model」：
    // 若改成逐字段 emit，每个使用它的页面都要把同一条 v-model 再写一遍，
    // 正是这个组件要消除的重复。因此这里显式关掉该规则（组件封装取向）。
    files: ['src/components/AppForm.vue'],
    rules: { 'vue/no-mutating-props': 'off' },
  },

  eslintConfigPrettier,
);
