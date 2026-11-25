# 自建同步服务运维

在包含 `compose.yaml` 的 `sync-server/` 目录执行以下命令。用同一个 Docker Compose 项目名管理这个实例。默认项目名是 `suojian`。首次部署见[项目 README](../README.md#自建同步服务器)。

## 确认服务就绪

```sh
docker compose ps
curl --fail --show-error https://你的同步域名/health
curl --fail --show-error https://你的同步域名/auth/config
```

在独立测试账号的 Windows 和 Android 客户端上用同一个 HTTPS 地址登录。先在 Windows 新建一张带附件的便签，在 Android 打开后核对正文和附件。两端断网后编辑同一段正文，恢复网络后核对冲突副本，两端选定并同步后应得到相同结果。再在一端移入回收站，确认另一端可以看到并恢复。测试账号至少要保留一张带附件的便签，供恢复验收使用。

## 备份

```sh
mkdir -p "$HOME/flashnote-backups"
sh data-backup.sh backup "$HOME/flashnote-backups/flashnote-$(date +%F-%H%M%S).tar.gz"
```

备份期间会短暂停止服务。脚本结束时会重新启动原先运行的服务。归档包含 `.env`、Casdoor 的配置与数据库、同步数据库与附件、Caddy 的状态与证书。归档内含账号配置、服务端同步密钥和证书私钥，请在访问受限的存储中加密保存，并另存一份到服务器之外。把对应的代码提交或发行标签和归档放在一起记录。升级或恢复期间避免同时执行其他 Compose 运维操作。

## 恢复或迁移到另一台服务器

1. 准备使用备份时相同版本部署文件的全新目录。使原 HTTPS 地址指向新服务器。登录标识绑定原地址，因此迁移时要保留 `SUOJIAN_ORIGIN`。恢复验收可仅通过测试客户端的名称解析将请求指向恢复实例，生产 DNS 可以继续指向原服务器。
2. 用一个新的 Compose 项目名，从可信的备份归档恢复。脚本要求目标目录中没有 `.env`、`.casdoor`，项目名下没有现有容器或卷。

   ```sh
   export COMPOSE_PROJECT_NAME=flashnote-recovered
   sh data-backup.sh restore /绝对路径/flashnote.tar.gz
   docker compose up -d --build
   ```

3. 先检查 `/health` 和 `/auth/config`。再用原测试账号在空白客户端资料库登录，同步并核对原便签、附件、分组和回收站状态。用第二个空白资料库同步一次新修改，确认恢复实例可以继续接收变更。

同一台服务器的标准端口只允许一个实例占用。切换前先停旧实例，保留它的目录与数据卷。若恢复中途失败，用另一份全新目录和新的项目名重试。

## 升级与回退

1. 先按上面的流程备份，记下旧版提交。把新版部署文件放到原目录，保留 `.env`、`.casdoor` 和 Compose 项目名。
2. 运行 `docker compose up -d --build`，检查服务启动状态、真实登录和双端同步。
3. 需要回退且新版本已写入数据时，使用旧版部署文件和升级前的备份恢复到新项目。安排切换前先导出升级后新增的便签，再导入恢复实例。
