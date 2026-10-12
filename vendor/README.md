# vendor — 离线构建资产

镜像构建默认会去 GitHub 取 swagger-ui 的发布包（`utoipa-swagger-ui` 的构建脚本）。
内网/离线环境把 zip 放到这里，同名即可：

```
swagger-ui-v5.32.6.zip
```

放好后 `docker build .` 会改用它（见 `Dockerfile` 里的 `SWAGGER_UI_DOWNLOAD_URL`）。
这个目录里的 zip 不入库（`.gitignore` 已忽略），所以本地开发机与 CI 都要自行准备。
