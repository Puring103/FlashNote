//! Additive fixtures for the explicitly selected A/B demonstration vault only.
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, io::Cursor, path::Path};
use suojian::{model::*, store::Store, sync};

const GROUPS: [(&str, &str, &str); 12] = [
    ("guide", "00 测试导览", "pin"),
    ("work", "01 工作项目", "file"),
    ("life", "02 生活清单", "list"),
    ("study", "03 学习摘录", "link"),
    ("idea", "04 灵感草稿", "pencil"),
    ("image", "05 图片相册", "image"),
    ("audio", "06 语音记录", "mic"),
    ("file", "07 文件资料", "file"),
    ("layout", "08 排版边界", "dot"),
    ("sync", "09 同步案例", "link"),
    ("trash", "10 回收恢复", "list"),
    ("empty", "11 空白分类", "dot"),
];

fn stable(key: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("suojian/local-test-cases/v1/{key}").as_bytes(),
    )
    .to_string()
}

fn open_pair(root: &Path) -> Result<(Store, Store)> {
    for device in ["device-a", "device-b"] {
        ensure!(
            root.join(device).join("sync-session.dpapi").is_file(),
            "cases / verify-demo 只接受两个设备均已真实登录的演示目录"
        );
    }
    let a = Store::open(root.join("device-a"))?;
    let b = Store::open(root.join("device-b"))?;
    let ca = sync::config(&a)?.context("A 未配置同步")?;
    let cb = sync::config(&b)?.context("B 未配置同步")?;
    ensure!(
        ca.vault_id == cb.vault_id && ca.url == cb.url && a.key.0[..] == b.key.0[..],
        "两个演示设备必须属于同一同步库"
    );
    Ok((a, b))
}

fn converge(a: &mut Store, b: &mut Store) -> Result<()> {
    for _ in 0..3 {
        sync::once(a)?;
        sync::once(b)?;
    }
    Ok(())
}

fn note(store: &Store, key: &str) -> Option<Note> {
    store.notes.iter().find(|n| n.id == stable(key)).cloned()
}

fn make(key: &str, group: &str, text: &str) -> Note {
    let mut n = Note::new();
    n.id = stable(key);
    n.group_id = stable(&format!("group/{group}"));
    n.set_markdown(text.into());
    n
}

fn add(store: &mut Store, key: &str, group: &str, text: &str) -> Result<bool> {
    if note(store, key).is_some() {
        return Ok(false);
    }
    store.save_note(make(key, group, text))?;
    Ok(true)
}

/// Reuse an existing fixture asset, so repeated installs add no orphan assets.
fn asset(store: &Store, name: &str, bytes: &[u8]) -> Result<String> {
    for n in &store.notes {
        for block in n.blocks.iter().chain(&n.media_archive) {
            if let Block::File {
                asset, name: found, ..
            }
            | Block::Image {
                asset, name: found, ..
            } = block
            {
                if found == name && store.asset(asset)? == bytes {
                    return Ok(asset.clone());
                }
            }
            if let Block::Audio { asset, .. } = block {
                if name == "测试语音.wav" && store.asset(asset)? == bytes {
                    return Ok(asset.clone());
                }
            }
        }
    }
    store.import_bytes(bytes)
}

fn picture(store: &mut Store, key: &str, text: &str, name: &str, bytes: &[u8]) -> Result<()> {
    if note(store, key).is_some() {
        return Ok(());
    }
    let mut n = make(key, "image", text);
    n.blocks.push(Block::Image {
        id: id(),
        asset: asset(store, name, bytes)?,
        name: name.into(),
    });
    store.save_note(n)
}

fn file(store: &mut Store, key: &str, text: &str, name: &str, bytes: &[u8]) -> Result<()> {
    if note(store, key).is_some() {
        return Ok(());
    }
    let mut n = make(key, "file", text);
    n.blocks.push(Block::File {
        id: id(),
        asset: asset(store, name, bytes)?,
        name: name.into(),
    });
    store.save_note(n)
}

fn pattern(width: u32, height: u32) -> Result<Vec<u8>> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        if (x / 40 + y / 40) % 2 == 0 {
            image::Rgb([52, 125, 120])
        } else {
            image::Rgb([235, 218, 180])
        }
    });
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn media_cases(a: &mut Store) -> Result<()> {
    let jpg = include_bytes!("../../../../../assets/demo/desk.jpg");
    picture(
        a,
        "image-photo",
        "## CASE-I01 · 图片与正文\n\n随应用附带的钢笔照片。检查缩略图、放大预览和重新打开。",
        "测试钢笔照片.jpg",
        jpg,
    )?;
    picture(
        a,
        "image-shared",
        "## CASE-I02 · 复用同一图片\n\n这张与 I01 引用相同素材 ID。两张便签都能打开，但空间只计一份附件。",
        "测试钢笔照片.jpg",
        jpg,
    )?;
    picture(
        a,
        "image-wide",
        "## CASE-I03 · 横向宽图\n\n1200 × 180 的测试格子。检查窄窗口和图片比例。",
        "横向测试.png",
        &pattern(1200, 180)?,
    )?;
    picture(
        a,
        "image-tall",
        "## CASE-I04 · 竖向长图\n\n240 × 900 的测试格子。检查卡片内滚动和放大。",
        "竖向测试.png",
        &pattern(240, 900)?,
    )?;

    let wav = include_bytes!("../../../../../assets/demo/sample.wav");
    let reader = hound::WavReader::new(Cursor::new(wav))?;
    let duration_ms = reader.duration() as u64 * 1000 / reader.spec().sample_rate as u64;
    for (key, text, state, transcript) in [
        (
            "audio-ready",
            "## CASE-A01 · 已转写语音\n\n展开文本并播放；转写文本为演示文字，不需要麦克风。",
            AudioState::Ready,
            Some("测试转写：周五整理离线便签，联网后检查图片和文件是否完整。".into()),
        ),
        (
            "audio-recorded",
            "## CASE-A02 · 未转写语音\n\n已保存有效音频，尚未转写。可测试播放；真实转写需要本机模型。",
            AudioState::Recorded,
            None,
        ),
        (
            "audio-failed",
            "## CASE-A03 · 转写失败状态\n\n人工设置的失败状态，用于检查提示与重试入口。音频本身有效，仍应能播放。",
            AudioState::Failed("演示：模型未安装，可配置模型后重试".into()),
            None,
        ),
    ] {
        if note(a, key).is_none() {
            let mut n = make(key, "audio", text);
            n.blocks.push(Block::Audio {
                id: id(),
                asset: asset(a, "测试语音.wav", wav)?,
                duration_ms,
                text: transcript,
                state,
            });
            a.save_note(n)?;
        }
    }

    file(
        a,
        "file-txt",
        "## CASE-F01 · 中文文本附件\n\n打开后应看到两行中文；文件名带空格与中文。",
        "测试说明 中文 空格.txt",
        "第一行：这是测试附件。\r\n第二行：修改前请另存副本。\r\n".as_bytes(),
    )?;
    file(
        a,
        "file-csv",
        "## CASE-F02 · CSV 附件\n\n三行测试数据，可用表格程序打开。",
        "空间统计.csv",
        "\u{feff}项目,数量,备注\r\n图片,4,含一份复用\r\n语音,3,同一测试音频\r\n".as_bytes(),
    )?;
    file(
        a,
        "file-json",
        "## CASE-F03 · JSON 附件\n\n检查结构化文件保存和系统打开。",
        "测试配置.json",
        b"{\"demo\":true,\"devices\":[\"A\",\"B\"],\"version\":1}\n",
    )?;
    file(
        a,
        "file-empty",
        "## CASE-F04 · 零字节文件\n\n文件内容为空，但加密封装仍占少量空间。文件卡片应正常显示。",
        "空文件.txt",
        b"",
    )?;
    file(
        a,
        "file-large",
        "## CASE-F05 · 1 MiB 二进制附件\n\n用于观察同步与容量变化。扩展名为 bin，不是可执行程序。",
        "容量测试-1MiB.bin",
        &vec![0x5a; 1024 * 1024],
    )?;
    file(
        a,
        "file-name",
        "## CASE-F06 · 较长中文文件名\n\n检查卡片截断、悬停和系统打开；文件内容是普通 UTF-8 文本。",
        "这是一份用于检查卡片文件名称显示与自动换行的中文测试附件 2026-11-12.txt",
        "长文件名测试成功。".as_bytes(),
    )?;

    if note(a, "mixed").is_none() {
        let mut n = make(
            "mixed",
            "idea",
            "## CASE-M01 · 混合素材\n\n- [x] 保存照片\n- [ ] 补充观察\n\n下面是图片、文字、录音和文件交替排列。",
        );
        let image_id = id();
        n.media_depths.insert(image_id.clone(), 1);
        n.blocks.push(Block::Image {
            id: image_id,
            asset: asset(a, "测试钢笔照片.jpg", jpg)?,
            name: "测试钢笔照片.jpg".into(),
        });
        n.blocks.push(Block::Markdown {
            id: id(),
            text: "图片之后的正文，应可继续编辑。\n\n> 下一张是语音卡片。".into(),
        });
        n.blocks.push(Block::Audio {
            id: id(),
            asset: asset(a, "测试语音.wav", wav)?,
            duration_ms,
            text: Some("混合素材演示转写。".into()),
            state: AudioState::Ready,
        });
        n.blocks.push(Block::File {
            id: id(),
            asset: asset(
                a,
                "测试说明 中文 空格.txt",
                "第一行：这是测试附件。\r\n第二行：修改前请另存副本。\r\n".as_bytes(),
            )?,
            name: "测试说明 中文 空格.txt".into(),
        });
        n.blocks.push(Block::Markdown {
            id: id(),
            text: "素材之后的尾段：这里也可以继续输入。".into(),
        });
        a.save_note(n)?;
    }
    Ok(())
}

fn static_cases(a: &mut Store, b: &mut Store) -> Result<()> {
    let rows = [
        (
            "guide",
            "guide",
            "## 从这里开始 · 测试导览\n\n这是独立的双设备测试库，可以自由编辑。\n\n- [ ] 切换分类，检查颜色与图标\n- [ ] 搜索 CASE-I 找图片，CASE-A 找录音，CASE-F 找附件\n- [ ] 编辑后退出并重开，确认自动保存\n- [ ] 在设置中检查同步状态和空间\n- [ ] 从设置退出，再切换设备 B\n- [ ] 打开回收站，恢复一张测试便签\n\n分类 09 含真实离线同步产生的合并与冲突结果。默认分类保留上一轮的 5 张原始演示便签。\n\n完整步骤见项目 docs/本机测试案例清单.md。重新补充案例会跳过已有便签，保留你的编辑和回收状态。",
        ),
        (
            "work-meeting",
            "work",
            "## CASE-W01 · 项目周会\n\n**本周目标**：把离线记录在不同设备之间完整带走。\n\n1. 电脑整理需求\n2. 手机补充现场记录\n3. 联网后复核附件\n\n> 决定：先完成个人设备同步。\n\n- [x] 梳理数据结构\n- [ ] 体验断网恢复\n- [ ] 记录测试问题",
        ),
        (
            "work-project",
            "work",
            "## CASE-W02 · 发布检查\n\n- 客户端\n    - 正文自动保存\n    - 图片与语音\n- 服务端\n    - 密文存储\n    - 容量统计\n\n`CASE-W02` 是搜索关键词；可将此便签移动到生活分类，再到另一设备验证。",
        ),
        (
            "life-shopping",
            "life",
            "## CASE-L01 · 采购清单\n\n- [x] 牛奶\n- [ ] 苹果 6 个\n- [ ] 咖啡豆\n    - 中度烘焙\n    - 250 g\n- [ ] 一盆绿植\n\n点击复选框，重开后状态应保留。",
        ),
        (
            "life-travel",
            "life",
            "## CASE-L02 · 周末出行\n\n09:00 出发 → 公园 → 午餐 → 回家。\n\n**预算**：¥200.50（纯演示数字）。\n\n- 雨伞\n- 水杯\n- 充电宝\n\n地点均为泛指，无真实行程。",
        ),
        (
            "study-format",
            "study",
            "## CASE-S01 · 阅读摘录\n\n> 先把想法留下来，再整理成结构。\n\n**重点**、*强调*、~~过期结论~~ 与 `inline code`。\n\n### 下一步\n\n1. 回顾\n2. 归纳\n3. 实践\n\n[参考：Rust 官方网站](https://www.rust-lang.org)",
        ),
        (
            "study-code",
            "study",
            "## CASE-S02 · 代码与缩进\n\n```rust\nfn main() {\n    let message = \"离线记录\";\n    println!(\"{message}\");\n}\n```\n\n- 一级\n    - 二级\n        - 三级\n\n代码里含 `< > & \"` 等符号。",
        ),
        ("idea-short", "idea", "灵感：把记录入口放在手边。 CASE-B01"),
        (
            "idea-link",
            "idea",
            "## CASE-B02 · 关联便签\n\n这张便签已关联工作周会 W01。可以检查关联展示和跳转。\n\n在这里补充跨分类的想法。",
        ),
        (
            "layout-unicode",
            "layout",
            "## CASE-E01 · 中文与 Unicode\n\n中文 English 日本語 한국어。\n\n🙂 🚀 🌱 🧑‍💻 café e\u{301} ① ② ③\n\n全角：ＡＢＣ１２３；半角：ABC123。\n\n标点：‘单引号’ “双引号” —— …… <tag> & 符号。\n\n搜索词：星河测试 / Unicode / café。",
        ),
        (
            "layout-lists",
            "layout",
            "## CASE-E02 · 混合列表\n\n1. 第一项\n2. 第二项\n    - 子项甲\n    - 子项乙\n3. 第三项\n\n- [ ] 未完成\n- [x] 已完成\n\n> 引用第一段\n>\n> 引用第二段\n\n### 三级标题\n\n正文末尾。",
        ),
        (
            "layout-twin-a",
            "layout",
            "## CASE-E03 · 同名便签\n\n这是第一份内容，编号 A。相同标题不应让两张便签互相覆盖。",
        ),
        (
            "layout-twin-b",
            "layout",
            "## CASE-E03 · 同名便签\n\n这是第二份内容，编号 B。与编号 A 拥有不同 ID。",
        ),
        (
            "sync-a",
            "sync",
            "## CASE-N01 · 设备 A 离线新增\n\n这张在 A 本地创建，B 尚未拉取时不存在。最终两台设备都应能看见。",
        ),
        (
            "sync-b",
            "sync",
            "## CASE-N02 · 设备 B 离线新增\n\n这张在 B 本地创建，A 尚未拉取时不存在。最终两台设备都应能看见。",
        ),
        (
            "trash-restore",
            "trash",
            "## CASE-R01 · 待恢复的文字\n\n此便签初始位于回收站。恢复后应重新出现在分类 10，并同步到另一设备。",
        ),
        (
            "trash-active",
            "trash",
            "## CASE-R02 · 可手动删除的便签\n\n可把我移到回收站，再恢复。检查两个设备的删除状态一致。\n\n移入回收站仍占用存储空间。",
        ),
    ];
    for (key, group, text) in rows {
        let store = if key == "sync-b" { &mut *b } else { &mut *a };
        if add(store, key, group, text)? {
            let mut n = note(store, key).unwrap();
            if key == "trash-restore" {
                n.deleted_at = Some(now());
                store.save_note(n)?;
            } else if key == "idea-link" {
                n.links.push(stable("work-meeting"));
                store.save_note(n)?;
            }
        }
    }
    if note(a, "layout-long").is_none() {
        let mut text =
            "## CASE-E04 · 长文滚动\n\n用于检查长便签、卡片高度和窄窗口换行。".to_string();
        for i in 1..=18 {
            text.push_str(&format!("\n\n### 第 {i} 节\n\n记录背景、决定和下一步。这里是较长的中文段落，用于检查阅读与编辑时的自动换行。把窗口变窄、滚动到底部、修改一句话，再重新打开确认位置与内容。\n\n- [ ] 第 {i} 节跟进\n\n> 长文测试段落 {i}。"));
        }
        add(a, "layout-long", "layout", &text)?;
    }
    add(
        a,
        "layout-word",
        "layout",
        &format!(
            "## CASE-E05 · 长英文与换行\n\n{}\n\n上面的长单词用于观察卡片横向边界。\n\n末尾正常中文。",
            "abcdefghij".repeat(24)
        ),
    )?;
    if add(
        a,
        "history",
        "work",
        "## CASE-H01 · 本机历史\n\n第一版：记录需求。",
    )? {
        for text in [
            "## CASE-H01 · 本机历史\n\n第二版：补充附件检查。",
            "## CASE-H01 · 本机历史\n\n第三版：确认离线同步。\n\nA 本机保存了三版历史。云端只同步最新快照，B 不会补齐 A 的旧版本。",
        ] {
            let mut n = note(a, "history").unwrap();
            n.set_markdown(text.into());
            a.save_note(n)?;
        }
    }
    media_cases(a)?;
    if note(a, "trash-image").is_none() {
        let mut n = make(
            "trash-image",
            "trash",
            "## CASE-R03 · 回收站中的图片\n\n恢复后应仍能打开照片。与图片分类共用同一素材，回收状态不应破坏素材。",
        );
        n.blocks.push(Block::Image {
            id: id(),
            asset: asset(
                a,
                "测试钢笔照片.jpg",
                include_bytes!("../../../../../assets/demo/desk.jpg"),
            )?,
            name: "测试钢笔照片.jpg".into(),
        });
        n.deleted_at = Some(now());
        a.save_note(n)?;
    }
    Ok(())
}

/// These are produced by the actual merge path, not fabricated conflict badges.
fn sync_cases(a: &mut Store, b: &mut Store) -> Result<()> {
    for (key, base) in [
        (
            "merge",
            "## CASE-C01 · 不同段落自动合并\n\n电脑待修改段\n\n中间锚点保持不变\n\n手机待修改段",
        ),
        ("overlap", "## CASE-C02 · 同段修改\n\n共同起点"),
        ("delete-edit", "## CASE-C03 · 删除与离线编辑\n\n共同起点"),
        (
            "field-merge",
            "## CASE-C04 · 移动分类与编辑正文\n\n共同起点",
        ),
    ] {
        // Existence is the checkpoint: never reset a case the user has touched.
        if note(a, key).is_some() || note(b, key).is_some() {
            continue;
        }
        a.save_note(make(key, "sync", base))?;
        converge(a, b)?;
        let mut an = note(a, key).unwrap();
        let mut bn = note(b, key).unwrap();
        match key {
            "merge" => {
                an.set_markdown(base.replace("电脑待修改段", "电脑离线修改：第一段已保留"));
                bn.set_markdown(base.replace("手机待修改段", "手机离线修改：最后一段已保留"));
            }
            "overlap" => {
                an.set_markdown("## CASE-C02 · 同段修改\n\n电脑版本：周六整理照片。".into());
                bn.set_markdown(
                    "## CASE-C02 · 同段修改\n\n手机版本：周日整理照片。两份内容都要保留。".into(),
                );
            }
            "delete-edit" => {
                an.deleted_at = Some(now());
                bn.set_markdown("## CASE-C03 · 删除与离线编辑\n\n手机断网期间补充的内容仍保留在冲突副本；原便签在回收站。".into());
            }
            "field-merge" => {
                an.group_id = stable("group/work");
                bn.set_markdown("## CASE-C04 · 移动分类与编辑正文\n\n电脑将我移入工作项目；手机更新了这段正文。两项修改均已合并。".into());
            }
            _ => unreachable!(),
        }
        a.save_note(an)?;
        b.save_note(bn)?;
        converge(a, b)?;
    }
    for (key, label, choice) in [
        (
            "choice-local",
            "CASE-C05 · 已选择保留本地",
            suojian::store::ConflictChoice::Local,
        ),
        (
            "choice-cloud",
            "CASE-C06 · 已选择保留云端",
            suojian::store::ConflictChoice::Cloud,
        ),
        (
            "choice-both",
            "CASE-C07 · 已选择两份都保留",
            suojian::store::ConflictChoice::Both,
        ),
    ] {
        if note(a, key).is_some() || note(b, key).is_some() {
            continue;
        }
        a.save_note(make(key, "sync", &format!("## {label}\n\n共同起点")))?;
        converge(a, b)?;
        let mut an = note(a, key).unwrap();
        let mut bn = note(b, key).unwrap();
        an.set_markdown(format!("## {label}\n\n云端内容：来自设备 A。"));
        bn.set_markdown(format!("## {label}\n\n本地内容：来自设备 B。"));
        a.save_note(an)?;
        b.save_note(bn)?;
        converge(a, b)?;
        let (fork, cloud) = b.conflict_pair(&stable(key)).context("测试冲突未生成")?;
        b.resolve_conflict(&fork.id, &fork.revision, &cloud.revision, choice)?;
        converge(a, b)?;
    }
    Ok(())
}

fn image_download_cases(a: &mut Store) -> Result<()> {
    for (key, text) in [
        (
            "image-remote-ok",
            "## CASE-I05 · 网页图片自动下载\n\n图片前的文字。 ![本机测试图片](http://127.0.0.1:47832/redirect.jpg) 图片后的文字。\n\n打开后自动下载到本机，切换设备也能显示。",
        ),
        (
            "image-remote-fail",
            "## CASE-I06 · 下载失败与重试\n\n![不存在的测试图片](http://127.0.0.1:47832/missing.jpg)\n\n这个链接故意返回 404。检查失败占位、重试按钮，退出重开应仍保留。",
        ),
        (
            "image-paste-guide",
            "## CASE-I07 · 粘贴网页图片\n\n打开 http://127.0.0.1:47832/page ，在浏览器中复制含图片的正文，再粘贴到这里，检查前后文字和图片位置。\n\n也可以粘贴：\n\n```markdown\n前面 ![图片](http://127.0.0.1:47832/ok.jpg) 后面\n```",
        ),
    ] {
        add(a, key, "image", text)?;
    }
    Ok(())
}

fn snapshot(store: &Store) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut objects = BTreeMap::new();
    for n in &store.notes {
        objects.insert(n.id.clone(), serde_json::to_value(n)?);
    }
    for g in &store.groups {
        objects.insert(g.id.clone(), serde_json::to_value(g)?);
    }
    Ok(objects)
}

fn inspect(a: &Store, b: &Store) -> Result<serde_json::Value> {
    ensure!(
        snapshot(a)? == snapshot(b)?,
        "A/B 对象尚未一致，请关闭 GUI 后执行同步"
    );
    let mut refs = std::collections::BTreeSet::new();
    let mut blocks = BTreeMap::<&str, usize>::new();
    for n in &a.notes {
        for block in n.blocks.iter().chain(&n.media_archive) {
            let (kind, aid) = match block {
                Block::Image { asset, .. } => ("image", asset),
                Block::Audio { asset, .. } => ("audio", asset),
                Block::File { asset, .. } => ("file", asset),
                Block::Markdown { .. } | Block::RemoteImage { .. } => continue,
            };
            *blocks.entry(kind).or_default() += 1;
            ensure!(a.asset(aid)? == b.asset(aid)?, "附件在 A/B 上不一致");
            for store in [a, b] {
                let wire = std::fs::read(store.root.join("sync-assets").join(aid))?;
                ensure!(
                    suojian_core::local::open_asset(&store.key, aid, &wire)? == store.asset(aid)?,
                    "附件传输缓存认证失败或与本地原件不一致"
                );
            }
            if kind == "image" {
                image::load_from_memory(&a.asset(aid)?)?;
            }
            if kind == "audio" {
                hound::WavReader::new(Cursor::new(a.asset(aid)?))?;
            }
            refs.insert(aid.clone());
        }
    }
    let usage = sync::usage(a)?;
    ensure!(
        usage.used_bytes == usage.asset_bytes + usage.object_bytes,
        "云端空间账本不平"
    );
    ensure!(
        usage.object_count as usize == a.notes.len() + a.groups.len(),
        "云端对象数不一致"
    );
    let mut asset_bytes = 0;
    for aid in &refs {
        asset_bytes += std::fs::metadata(a.root.join("sync-assets").join(aid))?.len();
    }
    ensure!(
        usage.asset_count as usize >= refs.len() && usage.asset_bytes >= asset_bytes,
        "云端附件计数不一致"
    );
    Ok(serde_json::json!({
        "device_a":a.root,"device_b":b.root,"notes":a.notes.len(),
        "active_notes":a.notes.iter().filter(|n|n.deleted_at.is_none()).count(),
        "trash_notes":a.notes.iter().filter(|n|n.deleted_at.is_some()).count(),
        "conflict_copies":a.notes.iter().filter(|n|n.conflict_of.is_some()).count(),
        "unresolved_conflicts":a.notes.iter().filter(|n|n.conflict_of.is_some()&&!n.conflict_resolved&&n.deleted_at.is_none()).count(),
        "pending_images":a.notes.iter().flat_map(|n|&n.blocks).filter(|b|matches!(b,Block::RemoteImage{..})).count(),
        "groups":a.groups.len(),"referenced_assets":refs.len(),"media_blocks":blocks,
        "groups_detail":a.groups.iter().map(|g|serde_json::json!({"name":g.name,"icon":g.icon,"color":format!("#{:06x}",g.color),"active":a.notes.iter().filter(|n|n.group_id==g.id&&n.deleted_at.is_none()).count(),"trash":a.notes.iter().filter(|n|n.group_id==g.id&&n.deleted_at.is_some()).count()})).collect::<Vec<_>>(),
        "usage":usage,"verification":"identical objects, authenticated assets, valid images/audio, balanced usage"
    }))
}

pub fn verify(root: &Path) -> Result<()> {
    let (a, b) = open_pair(root)?;
    println!("{}", serde_json::to_string_pretty(&inspect(&a, &b)?)?);
    Ok(())
}

pub fn install(root: &Path) -> Result<()> {
    let (mut a, mut b) = open_pair(root)?;
    converge(&mut a, &mut b)?;
    // Create both devices' groups while offline; each side owns a disjoint subset.
    for (index, (key, name, icon)) in GROUPS.iter().enumerate() {
        let gid = stable(&format!("group/{key}"));
        if a.groups.iter().chain(&b.groups).any(|g| g.id == gid) {
            continue;
        }
        let store = if index % 2 == 0 { &mut a } else { &mut b };
        store.save_group(Group {
            order: 0,
            deleted: false,
            id: gid,
            name: (*name).into(),
            color: GROUP_COLORS[index],
            icon: (*icon).into(),
        })?;
    }
    converge(&mut a, &mut b)?;
    static_cases(&mut a, &mut b)?;
    converge(&mut a, &mut b)?;
    sync_cases(&mut a, &mut b)?;
    image_download_cases(&mut a)?;
    converge(&mut a, &mut b)?;
    let report = inspect(&a, &b)?;
    std::fs::write(
        root.join("cases-report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_store::open_store;

    #[test]
    fn adding_cases_again_preserves_edits_trash_history_and_asset_count() {
        let temp = tempfile::tempdir().unwrap();
        let mut a = open_store(temp.path().join("a")).unwrap();
        let mut b = open_store(temp.path().join("b")).unwrap();
        for store in [&mut a, &mut b] {
            for (i, (key, name, icon)) in GROUPS.iter().enumerate() {
                store
                    .save_group(Group {
                        order: 0,
                        deleted: false,
                        id: stable(&format!("group/{key}")),
                        name: (*name).into(),
                        color: GROUP_COLORS[i],
                        icon: (*icon).into(),
                    })
                    .unwrap();
            }
        }
        static_cases(&mut a, &mut b).unwrap();
        let mut edited = note(&a, "work-meeting").unwrap();
        edited.set_markdown("用户测试后保留的内容".into());
        edited.deleted_at = Some(now());
        a.save_note(edited).unwrap();
        let before = snapshot(&a).unwrap();
        let revisions: i64 = rusqlite::Connection::open(a.root.join("notes.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
            .unwrap();
        let assets = std::fs::read_dir(a.root.join("assets")).unwrap().count();
        static_cases(&mut a, &mut b).unwrap();
        assert_eq!(before, snapshot(&a).unwrap());
        assert_eq!(
            revisions,
            rusqlite::Connection::open(a.root.join("notes.sqlite3"))
                .unwrap()
                .query_row::<i64, _, _>("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
                .unwrap()
        );
        assert_eq!(
            assets,
            std::fs::read_dir(a.root.join("assets")).unwrap().count()
        );
        assert_eq!(
            note(&a, "file-empty").unwrap().blocks.iter().find_map(|b| {
                if let Block::File { asset, .. } = b {
                    Some(a.asset(asset).unwrap())
                } else {
                    None
                }
            }),
            Some(vec![])
        );
    }
}
