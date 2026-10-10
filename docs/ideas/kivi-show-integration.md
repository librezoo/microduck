# 与 Kivi Studio 联调：多轨歌曲包、连接与分工

> 状态：设计，**尚未实现**。`ideas/` 下的文档是还没成为决策的设计（见 `docs/README.md`）。
> 它建立在 [`singing-performance.md`](singing-performance.md) 之上：那份文档定义了机器侧怎样把"唱了什么"变成嘴和头颈的动作，
> 本文定义 **App 怎样把一首歌交给鸭子、鸭子怎样把每一轨接起来**。
> 证据标签同前：**［已确认］** 读过代码/文档或跑过 · **［决策］** 推荐 · **［假设］** 暂定 · **［待验证］** 无证据。

## 0. 先说结论

1. **一首歌 = 一个"表演包"（show），由 manifest + 若干轨组成。** 推荐 4 类轨：
   `duck`（鸭声：音符谱，必需）、`motion`（动作：串行动作片段，必需）、`audio`（播放用音频，伴奏或混音）、
   `expression`（嘴/头补丁，可选）。嘴和头颈的动作**不由 App 生成**，由机器侧 `singer` 从 `duck` 轨编译。
2. **传输：命令与状态走一条常连接，大文件走 HTTP 分轨上传、带哈希、最后一次原子提交。**
   这样改动作只需重传几 KB，不必重传 20 MB 的音频。
3. **Kivi 已有的 LAN 适配器协议不丢**：机器侧新写的 `showd` 先**原样实现它**（`/pair`、`/info`、`/snapshot`、`PUT /songs`、`/control`），
   Studio 不改一行就能连上官方鸭子；多轨是在它旁边**加**的端点，而不是另起一套。
4. **App 的预览和鸭子的动作用同一份代码**：把 `singer`（纯 Rust，无 IO）编译成 WASM 给 Studio 预览——和 Kivi 现在对 `sounds` 做的事一样。
   今天 Studio 的虚拟鸭嘴是"与音符无关的交替开合"（`singingMouthStep`），上线后预览与真机会不一致，这是要先消除的偏差。
5. **音频只有一个写入者。** 鸭子的 PCM 是单客户端（`robotd/src/sound.rs`），所以不能让 Studio 的 Python 服务（`mpv`）和 `robotd` 各开一个播放。
   声音必须由**一个进程**播，且它必须是时钟主：动作、嘴头、声音共用一个起点。

## 1. 现状（两边都读过）

### 1.1 Kivi 侧［已确认：读自 `cyber-zoo/kivi-studio` 的 `main` 与 `codex/mobile-single-page`、`cyber-zoo/kivi-device-protocol`］

| 项 | 现状 |
|---|---|
| 编辑模型 | `SoundEvent{ startBeat, durationBeats, voice(ah/eh/ee/oh/oo/mm), midiNote, volume, continuesPrevious }`；`ActionEvent{ actionId, startMs, durationMs, source }`，动作严格串行；速度 `tempoBpm` 恒定 |
| 声音 | 浏览器里用 WASM 跑官方 `sounds` 合成器渲染歌声（固定种子），并加默认的音间断句；**上传的是已混好的单个 WAV** |
| LAN 适配器 | 在 `codex/mobile-single-page`：`http://<鸭>:8765/api/v1` — `POST /pair`、`GET /info`、`GET /snapshot`、`PUT /songs/{id}`（`audio/wav`，带 `X-Base-Revision`）、`POST /control`（play/pause/resume/stop/seek/volume）；Bearer 令牌；快照带 `bootId/sequence/version` 防乱序 |
| 配网 | 自有 BLE GATT（UUID `b9b6…`）：发现→`bind.begin`（鸭子嘎嘎两声）→按键确认→Wi-Fi 扫描/连接→`network.endpoint` 回地址 |
| 设备侧 | `device/audio-service`、`device/studio-service`（Python，**Orange Pi**）：收歌、`mpv`/ALSA 播放、曲库；**没有动作、舵机、robotd 接口** |
| 协议仓库 | `kivi-device-protocol` 的 schema 还是 **`ki`/`ka`** 音节、整份编排 base64 内联上传（上限 1 MiB）、`start_score` 带 `startAtUnixMs` ——**与 Studio 现状已经脱节** |
| 动作 | 15 个动作，来自官方策略，但**预览是手工插值，不是策略仿真**；他们自己的审计（`docs/motion-hardware-audit.md`）指出坐姿接其他动作有 77° 的目标跳变、`sit_toggle` 是有状态操作等 |
| 联调状态 | 与整机鸭子**尚未联调**；README 自己写明"Orange Pi 实机安装及出声仍待验证" |

### 1.2 鸭子侧［已确认：本仓库与已部署的板子］

| 项 | 现状 |
|---|---|
| 进程 | `robotd`（控制环，唯一的关节所有者）、`btd`（BLE）、`mediad`（相机 + WebRTC，8080/8443）、`configd`、`updaterd` |
| 对外接口 | JSON-RPC（`duck-ipc-proto`）：本机 unix socket；BLE 走 `btd` 的子集；WebRTC `control` 数据通道走 `mediad` 的另一个子集；**没有 Wi-Fi/HTTP 的歌曲服务** |
| 动作 | `robot.do <skill>`（roulade、kick、ground_pick、`sit_toggle`…，名字由机器人自己的策略集决定）；`robot.policies` 可查询 |
| 唱歌 | 本轮新增的 `robot.sing` + `singer`：嘴/头从表演描述生成，**已在真机坐姿下跑通**（头方向、声音、同步已由你确认） |
| 音频 | `robotd` 通过 `aplay` 管道独占 PCM；只有音效库和合成器，**没有播放任意 WAV 的路径** |
| 配网 | `btd` + `configd`：BLE 配对（PIN）、Wi-Fi 配置；与 Kivi 的 Studio GATT 是两套，且 Kivi 文档承认"**正式产品需与官方 btd 统一管理配对策略**" |

**冲突点：** Kivi 的设备服务假设自己独占蓝牙配对代理和声卡，官方鸭子栈已经有 `btd` 和 `robotd` 占着它们。所以"把 Kivi 的 Python 服务装上板子"
在联调里能当临时手段，但**不能是终态**。

## 2. 分轨与包格式

### 2.1 为什么是这几轨［决策］

你提出的"伴奏、鸭声、动作"是对的，补两点：

- **鸭声要拆成"谱"和"渲染"。** 谱（音符、元音、音高、响度、续音标记）是**真值**：它同时驱动发声、嘴、头。渲染出来的 WAV 只是它的一个产物。
  只传 WAV，机器侧就拿不到音节边界，嘴头只能退化成响度跟随（上一份文档里测过：起音准时 25/83 对 82/83）。
- **嘴和头不该由 App 生成。** 它们依赖机构限速、延迟补偿、身体状态仲裁，这些全在机器侧；App 只在需要手工微调时给一个补丁轨。

| 轨 | 必需 | 内容 | 谁产生 | 机器侧由谁消费 |
|---|---|---|---|---|
| `duck` | ✔ | 音符谱：`startBeat / durationBeats / voice / midiNote / volume / continuesPrevious`，另有 `tempoBpm`；可选 `phrases`、`sections`、`accents` 标注 | App 编辑器直接导出 | `singer`（嘴、头颈）；以及机器人自己的合成器（用**自己**的音色唱） |
| `motion` | ✔ | 串行动作片段 `actionId / startMs / durationMs / source` | App 编辑器 | 动作执行器（§4.3）：映射到 `robot.do` 等 |
| `audio` | 二选一（见下） | 播放用音频：`role: "mix"`（伴奏+鸭声已混好）或 `role: "backing"`（只有伴奏） | App（今天已在做混音） | 播放器（独占 PCM） |
| `expression` | 可选 | 人工补丁：改一个音节边界、指定一处重音/换气/乐句划分（`singer` 的 patch 格式，见 `singing-performance.md` §4.5） | App 的编辑 UI（后期） | `singer` |

**`audio` 怎么用：**
- `role: "mix"`：机器侧只播它，`duck` 轨只用来驱动嘴和头（`mute`）。**这是第一版，和 Kivi 今天上传的东西一致**，机器侧无需混音。
- `role: "backing"`：机器侧自己合成鸭声（用本机的音色）并与伴奏混到同一个 PCM 写入里。这能让每只鸭子用**自己的**嗓音唱，也能单独调伴奏/鸭声音量。**第二版**，格式不用变。

### 2.2 时间基准［决策］

- **所有轨共用一个零点：包开始播放的瞬间。** `duck` 用拍（`startBeat`，按恒定 `tempoBpm` 换算），`motion` 用毫秒，`audio` 带 `startOffsetMs`（默认 0）。
  换算统一规则：`t_s = startBeat × 60 / tempoBpm`；**不允许变速**（`singer` 也不支持变速，与 Kivi 现状一致）。
- 起点用"相对启动时刻"而不是 Kivi 协议草案里的 `startAtUnixMs`：两端墙钟没有同步机制，相对时间由**设备端**打点，反而避免了手机时钟漂移。
  多只鸭子同步是以后的事（§6），到时用 `chorale` 那套节拍对齐，不靠墙钟。
- 设备端播放开始后，`robotd` 的单调时钟是主时钟；`audio_latency`、`mouth_latency`、`head_latency`（上一份文档的 `Profile`）把动作对齐到**声音到达房间**的时刻。

### 2.3 包的磁盘与线上表示［决策］

**线上：分轨传输，不是整包一个文件。** 理由：编辑循环里最常改的是 `motion` 和 `duck`（几 KB），`audio` 是几十 MB；整包 zip 会让每次改动都重传。
**导出/导入/分享：** 另有 `.kivi`（ZIP，内容与目录一致），只用于离线文件交换，不是传输单元。

```
shows/<showId>/
  manifest.json
  duck.score.json
  motion.json
  audio.wav                 # 或 backing.wav
  expression.patch.json     # 可选
```

```json
{
  "schema": "kivi.show/2",
  "showId": "uuid",
  "title": "学不会",
  "revision": "<manifest 的 sha256>",
  "tempoBpm": 78,
  "durationMs": 243477,
  "roles": [{ "roleId": "lead", "modelId": "microduck-v1" }],
  "tracks": {
    "duck":   { "file": "duck.score.json", "sha256": "…", "bytes": 5210 },
    "motion": { "file": "motion.json",     "sha256": "…", "bytes": 1840 },
    "audio":  { "file": "audio.wav", "role": "mix", "codec": "pcm_s16le",
                "sampleRate": 32000, "channels": 1, "startOffsetMs": 0,
                "sha256": "…", "bytes": 15600000 }
  }
}
```

- **音频格式：WAV PCM16 单声道，32 kHz**［决策］。扬声器是单个小喇叭；单声道 32 kHz 约 64 KB/s，4 分钟约 15 MB，
  对比今天样例（`xuebuhui-duck.wav`：48 kHz 立体声，46 MB）小 3 倍。WAV 因为鸭子侧已经有 `hound`/`aplay` 路径；Opus/FLAC 等以后再说（机器侧没有解码器）。
- **一个角色一个包**（`roles` 数组长度 1）。Kivi 的 schema 里有多角色，那是多鸭同步的预留；第一版设备端遇到多于一个角色直接拒绝并说明。
- 机器侧限制写进 `capabilities`（§3.3），App 在上传前就检查：最大包大小、最大事件数、支持的 `actionId`、是否支持 `backing` 混音。

## 3. 连接

### 3.1 分层［决策］

```
首次配网         BLE  ──→ btd + configd（官方，PIN 配对 + Wi-Fi 配置）          [见 §3.4]
日常控制/状态    Wi-Fi ─→ showd  /api/v1/events  (WebSocket, 常连接, 心跳)
大文件上传       Wi-Fi ─→ showd  PUT /api/v1/shows/{id}/tracks/{name}  (HTTP, 带哈希)
                          │
                          └─ 本机 unix socket JSON-RPC ──→ robotd（唯一关节所有者）
```

**新增一个守护进程 `showd`（Rust）**，而不是往 `robotd` 里塞 HTTP：`robotd` 是控制环，不该有对外网络面；`mediad` 是视频，职责也不对。
`showd` 只做三件事：**接收并校验包、保存曲库、把"播放"翻译成对 `robotd` 的调用**。这与 `btd` 对 `robotd` 的关系一样。

为什么不复用 WebRTC `control` 通道：它是 JSON 行，传文件要 base64（体积 +33%）且数据通道单条消息有上限；
Kivi 的适配器已经是 HTTP + XHR 上传（带进度）；常连接用 WebSocket 足够。WebRTC 留给相机和远程场景，二者不冲突。

### 3.2 `showd` 的 API［决策，是对 Kivi 现有 v1 的**加法**］

**保持不变（Studio 现有代码直接可用）：** `POST /pair`、`GET /info`、`GET /snapshot`、`POST /control`（play/pause/resume/stop/seek/volume）、`PUT /songs/{id}`（单 WAV，作为只有音频的"歌曲"，等价于只有 `audio` 轨的包）。

**新增：**

| 端点 | 作用 |
|---|---|
| `GET /api/v1/capabilities` | 设备能力：协议版本、`maxShowBytes`、`maxEvents`、支持的 `actionId`（来自 `robot.policies`）、是否支持 `backing` 混音、是否可 `mute`、当前姿态 |
| `GET /api/v1/shows` / `DELETE /shows/{id}` | 曲库；每项带 `revision` 和各轨哈希，App 据此只补传缺失/变化的轨 |
| `PUT /api/v1/shows/{id}/tracks/{name}` | 上传一条轨到**暂存区**；头部 `X-Sha256`；服务端边收边算哈希，不符即拒绝 |
| `PUT /api/v1/shows/{id}/manifest` | 上传 manifest（暂存） |
| `POST /api/v1/shows/{id}/commit` | 校验暂存区：所有轨哈希与 manifest 一致、`duck` 能编译成合法 `Performance`、`motion` 的每个 `actionId` 都被支持且前后姿态衔接合法；**通过才原子替换**，失败返回逐条原因；**半成品永远不可见**（与 Kivi 现有的"尚未发布的半首歌不会出现"一致） |
| `POST /api/v1/shows/{id}/play` | `{ "mute": bool, "prepare": bool, "mixer": "mix"\|"robot" }`；返回受理或拒绝原因 |
| `POST /control` 的 `stop`/`pause`/`resume`/`seek` | 沿用；语义见 §4.4 |
| `GET /api/v1/events`（WebSocket） | 常连接：服务端每个状态变化推送 `snapshot`（沿用 `bootId/sequence/version`），播放中每 100–200 ms 推进度；客户端每 5 s 心跳 `ping`，**30 s 无心跳则设备侧认为 App 离线，但不停止正在播放的歌**（Kivi 现有行为：断开不停播） |

**"已传送"的确认：** `commit` 成功的响应里带 `revision` 与各轨 `sha256`；`GET /shows` 与之核对。App 的"已存入鸭子"只在 `commit` 成功后显示。
**多轨播放控制按钮**对应：`play`（整首，所有轨一起）、`pause/resume`、`stop`、`seek`。第一版**不提供按轨单独播放**（只有"静音声音"这一个开关 `mute`），
因为动作轨与声音轨的单独播放没有明确的产品语义，且动作单播有安全问题（§4.3）。

### 3.3 错误与降级［决策］

`commit` 与 `play` 的拒绝都是**结构化原因**，App 直接翻译成中文提示：

| code | 含义 | App 怎么办 |
|---|---|---|
| `Incompatible` | 协议或 schema 版本不认识 | 提示更新 App / 鸭子 |
| `ActionUnsupported{actionId}` | 动作这只鸭子没有 | 在时间轴上标红 |
| `PoseMismatch{at, need, have}` | 动作衔接姿态不合法（如坐姿直接接走路，Kivi 自己的审计已指出） | 提示插入起身 |
| `NotEnabled` | 鸭子没有开启表演同意（`[chorale] accept`）或没有上电 | 提示去开启 |
| `Busy{reason}` | 正在合唱/theremin/另一首歌 | 等待或先停止 |
| `TooLarge` / `Corrupt` | 包超限/哈希不符 | 重传 |

### 3.4 配网与配对［决策，需要 Kivi 侧配合］

Kivi 的 Studio GATT 服务与官方 `btd` **不能共存为终态**（它们自己的文档承认 BlueZ 配对代理冲突）。推荐：

- **鸭子侧不装 Studio 的 BLE 服务，保留官方 `btd` + `configd`。** App 的 `ble.ts`/`controller.ts` 改为说官方 `duck-ipc-proto` 的 BLE 子集
  （`system.authenticate`、Wi-Fi 扫描/连接、`system.info`），官方手机 App 已经这么用（`app-path-design.md`）。
- 配对强度：官方现在的 PIN 是**所有鸭子共用的 `000000`**（`remote-webrtc.md` §4 自述），按 Kivi 文档的"机身按键确认 + 逐客户端凭据"要求**不够**。
  这是**两边共同的待办**：机身按键确认 + 逐客户端令牌应当做在 `btd`/`configd` 里，`showd` 只认这些令牌。
  在此之前，联调阶段用**临时的固定令牌**（`showd --token-file`），并明确标注为调试方案——不把它包装成安全。
- LAN 的 HTTP+Bearer 明文问题，Kivi 自己已标注；终态是与绑定关联的认证加密通道（TLS，设备证书）。**联调不依赖它，上线前必须解决。**

## 4. 机器侧：每一轨怎样接到执行接口

### 4.1 `duck` 轨 → 嘴、头颈（和声音）

新增 `singer::from_notes(score) -> Performance`（`from_score` 的兄弟）：

| Kivi 字段 | → `Performance` |
|---|---|
| `voice` | `Syllable.vowel`（6 个元音与 `singer::Vowel` **一一对应**，不是巧合——都来自官方 `sounds`） |
| `midiNote` | `pitch` |
| `volume` | `level` |
| `continuesPrevious` | `tie`（**一字多音**——上一份文档里"只有对齐歌词才知道"的东西，Kivi 的编辑器直接给了，这是个好消息） |
| 相邻音符的间隙 | 休止 → 闭嘴、换气、乐句边界（`breath ≥ 0.35 s` 划乐句） |
| 小节位置 + 音高峰值 + 长度 | 重音（沿用 `from_score` 的启发式；以后 App 可显式标注） |
| `Onset` | 谱里没有辅音，沿用推断：重复同音→`Closure`，音高变化→`Soft`，休止后→`None` |

段落（主歌/副歌）没有标注时用启发式；**建议 App 后期加一个可选的 `sections` 标注**，那样头部的"段落情绪"和"重复变体"才有依据。
**嘴头的生成不依赖 `audio` 轨。** 这也意味着：给定 `duck` 轨，我们不需要任何音频分析——这条路绕开了上一份文档里最难的 S3（分离/对齐）。
（对像 `xuebuhui-duck.wav` 那样只留下混音的历史产物仍然需要 S3；但**以后经过 Studio 产出的歌曲都有谱**。）

### 4.2 `audio` 轨 → 播放

- `role: "mix"`：`showd` 校验后存盘；播放时交给一个**统一的音频写入者**。写入者就是 `robotd` 现有的合成器 PCM 写入线程，
  给它加一个"播放 WAV 文件"的输入（`hound` 解码到 f32，按样本推进；与合成器输出共用一个 `aplay` 管道）。
  不用 `mpv`：独占 PCM 的前提下，第二个播放进程会抢设备，且无法给出时钟对齐的起点。
- `role: "backing"`（第二版）：同一个写入线程里把伴奏和本机合成的鸭声相加，各有音量。
- **音频延迟标定：** `audio_latency` 是 `Profile` 里已有的字段；联调阶段用 `robotctl sing --audio-latency-ms` 手工调，之后做回环测量（上一份文档的 S1）。

### 4.3 `motion` 轨 → 动作执行器（**最需要小心的一轨**）

这是 Kivi 自己的审计最担心的部分，也是我们安全责任最重的部分。

- **映射：** `actionId` → 机器人上 `robot.policies` 报告的技能名（`roulade`、`ground_pick`、`kick_left`、`sit_toggle`…）。
  Kivi 的 15 个动作里有的没有对应技能（如手工的"舞台鞠躬"）：它们在 `commit` 时被 `ActionUnsupported` 拒绝，**不会被悄悄忽略**。
- **姿态机：** 设备端维护 `{limp, sitting, standing, walking, busy}`；每个动作声明前置/后置姿态；`commit` 时静态检查整条时间轴，不合法则 `PoseMismatch`。
  `sit_toggle` 是**有状态**的（我自己上一轮就误触发过一次：它把坐姿切成了站姿），所以执行器必须读回姿态而不是"发一次就算数"，
  切换前先确认当前姿态，切换后等姿态确认再推进。
- **时间：** 动作片段的 `durationMs` 是 App 写的，但真实技能的时长由策略决定。执行器**以设备回报推进**（Kivi 审计里的建议 4），
  片段结束以"技能完成"为准，不是以计时器为准；`commit` 时拿到每个技能的实测最短/典型时长，超出标注的片段给警告。
- **与嘴头的仲裁：** 站立/走路/坐姿下嘴头允许（真机已测）；**技能执行期间（`ground_pick`、踢球、`roulade`）嘴头偏移关闭**（上一份文档 §7.2 的表）。
- **停止：** `stop` = 当前动作不再往下推进，嘴闭、头偏移归零（已实现）；**不自动站起**——自动起身本身是一个"动作"，在没有验证前不能当作安全的默认值。
  这与 Kivi 协议草案的 `returnToNaturalStand: true` 不同，是**要两边确认的产品决策**（§7）。
- **首批只开放这些动作（建议）：** `sit_toggle`（有姿态确认）、站立/待机、`roulade`。其余按 `motion-hardware-audit` 的顺序逐个验证后再放开。

### 4.4 播放状态机与控制语义

```
idle ─ play ─→ lead-in(1.5 s: 嘴闭、头吸气) ─→ playing ─ 结束 ─→ settling ─→ idle
                              playing ─ pause ─→ paused (嘴头回休止姿态、音频暂停、动作保持)
                                        ↑ resume (吸收偏差，不跳)
                              任一状态 ─ stop ─→ settling ─→ idle
```

- `pause/resume/seek` 对 `duck`/`audio` 轨已经由 `singer::Player` 保证不突跳（单测过）；对 `motion` 轨：**暂停 = 在当前动作的安全点保持**，
  不能在技能中途冻结策略（待验证，第一版 `pause` 仅在动作之间生效，动作进行中返回 `Busy`）。
- `seek` 同理：只允许落在动作边界上，否则拒绝（这直接回应 Kivi 审计第 5 条"音频拖动不能直接解释成实机姿态跳转"）。
- 播放前的 **preflight**：`play` 检查姿态、同意开关、电池（< 20% 拒绝）、温度、是否被合唱/theremin 占用，并返回结构化原因。

## 5. 分工：哪些在端侧，哪些在 App 侧

| 范围 | 端侧（本仓库） | App 侧（`kivi-studio`） | 共同（`kivi-device-protocol`） |
|---|---|---|---|
| **包格式** | 解析 manifest；`from_notes`；校验 | 生成 manifest 和各轨；导出/导入 `.kivi` | **schema 的唯一出处**：把 `kivi.show/2` 放进这个仓库，端侧与 App 都按它做契约测试 |
| **鸭声** | 机器人自己的合成器（本机音色）；`singer` 生成嘴头 | WASM 渲染鸭声（预览与 `mix` 音频）；**把 `singer` 编译成 WASM，让预览的嘴头与真机一致** | `duck` 轨的 schema，及"元音 6 种"的枚举 |
| **动作** | 动作执行器、姿态机、技能映射、`PoseMismatch` 校验 | 动作库、时间轴编辑；用设备 `capabilities` 标红不支持/衔接不合法的片段；**去掉手工插值预览的"可执行"暗示**（他们审计里已列） | `actionId` 命名约定；每个动作的前置/后置姿态表 |
| **音频** | 统一音频写入者（播放 WAV、混音、音量）；时钟主；延迟标定 | 混音/降采样为 32 kHz 单声道；音量 UI | 音频格式约定 |
| **连接** | `showd`（HTTP + WebSocket + 哈希校验 + 曲库）；对 `robotd` 的新调用 | `LanAudioAdapter` 扩展为 `ShowAdapter`：分轨上传、`commit`、增量补传、WebSocket 常连接与重连、进度条 | OpenAPI 文档 + 一份 fixtures（合法/非法包）|
| **配网/配对** | `btd`+`configd` 增加"机身按键确认 + 逐客户端令牌"；`showd` 认令牌 | `ble.ts` 改说官方 BLE 协议（弃用私有 GATT UUID）；绑定存储沿用 | 配对流程的状态机图 |
| **状态展示** | `robot.state.sing` 已有；`showd` 的事件 | 播放面板：已传送/传输中/播放中/错误；按钮 play/pause/resume/stop/seek/volume | 状态枚举与错误码 |
| **联调测试** | 在 `duck-sim`（MuJoCo 孪生，无头）里跑 `showd`；真机 | 在**不接真机**的 CI 里对 `showd --sim` 做契约测试；真机清单 | 黄金包 `golden-show/` |

**我们这边（端侧）不动的：** 编辑器、动作库、3D 预览、WASM 合成、配网界面。**Kivi 那边不动的：** 控制环、安全层、嘴头生成算法、策略。

## 6. 里程碑

| # | 目标 | 端侧工作 | App 侧工作 | 退出标准（可验证） |
|---|---|---|---|---|
| **K0** 契约先行（~3 天） | 两边对同一份包格式达成一致 | 出 `kivi.show/2` schema + 一个**黄金包**；`from_notes` 的契约测试 | 用现有编辑器导出一份真实歌曲（比如《学不会》的编辑草稿）的 `duck`/`motion` 轨 | 两边各自的测试都能读同一个黄金包；**这一步也解决"手里只有混音 WAV"的问题** |
| **K1** 最短通路（~1 周） | App 用现有按钮把一首歌传给真鸭子并让它出声 | `showd` v0：实现**现有 v1 API**（`/pair`、`/info`、`/snapshot`、`PUT /songs`、`/control`）；`robotd` 播放 WAV 的输入；固定调试令牌 | 不改（或只改默认端口/地址），在 Radxa 上联调 | App 里传一首歌、点播放，鸭子出声；暂停/继续/停止/音量可用；断网重连不丢状态 |
| **K2** 多轨（~2 周） | 嘴头跟着唱、动作按谱走 | 分轨上传 + `commit` 校验；`duck`→`singer`；动作执行器（先只开放 3 个动作）；`/events` WebSocket | `ShowAdapter`：增量上传、`capabilities` 标红、`singer` WASM 预览 | 同一首歌：App 预览与真机嘴头一致（同一代码）；`PoseMismatch` 能在上传时拦住；断开 App 不停播 |
| **K3** 同步与体验（~2 周） | 声音、嘴头、动作对齐 | 音频延迟回环标定；`pause/seek` 在动作边界；preflight | 播放面板的状态/错误提示、进度 | 在慢动作视频里起音偏差 ≤ 60 ms（设计目标）；整首 4 分钟无丢步 |
| **K4** 配对与安全（~2–3 周） | 去掉调试方案 | `btd`/`configd` 机身按键确认 + 逐客户端令牌；LAN 通道加密 | `ble.ts` 改说官方 BLE 协议；多设备绑定 | 没有固定令牌；未按键的客户端不能传歌或控制 |
| **K5** 扩展 | 伴奏混音（`role: "backing"`）、每只鸭用自己的嗓音；表演补丁轨；多鸭同步 | 混音写入者；`expression` 补丁 | 补丁编辑 UI | 按需 |

**关键路径：** K0 → K1 → K2 → K3 → K4。**K1 与 K0 之后可以并行：** 端侧的 `from_notes`/动作执行器（K2）和 App 侧的 `ShowAdapter`/`singer` WASM（K2）互不阻塞，用 `showd --sim` 对接。

## 7. 需要两边确认的决策

1. **`showd` 作为新守护进程，且端侧不装 Kivi 的 Python 服务**——同意吗？（临时联调可以先装 Python 服务验证 App，但它会和 `robotd` 抢声卡，所以只能在 K1 之前、`robotd` 不播声音时用。）
2. **配网改用官方 `btd`**，Kivi 的私有 GATT 弃用——这会让 App 侧改 `ble.ts`，需要你们评估成本。
3. **`stop` 是否自动站起？** 协议草案写了 `returnToNaturalStand: true`；我的建议是第一版**不**自动起身（起身本身是未验证的动作），只让嘴头归位。
4. **动作库的首批白名单**（建议 3 个）和每个动作的前置/后置姿态表，谁来维护？建议放在 `kivi-device-protocol`。
5. **`audio` 的第一版是 `mix`（App 混好）**，第二版才是 `backing`（机器人混音）——同意这个顺序吗？
6. **`kivi-device-protocol` 重写：** 删掉 `ki/ka`，改成 6 个元音，加上 `kivi.show/2`、capabilities、错误码。谁来做？

## 8. 本轮已就绪的、与这份计划相关的部分

- `singer` 的 `Syllable` 与 Kivi 的 `SoundEvent` 字段一一对应（含 `tie`↔`continuesPrevious`），`from_notes` 是一层很薄的转换。
- `robot.sing` 已支持 `mute`（只动嘴头）和 `performance`（外部描述），可以直接作为 K2 的内部调用，`showd` 只需把 `duck` 轨编译好后调用它。
- 真机已验证：坐姿下嘴头跟随、声音、延迟补偿（嘴约 150 ms、头约 250 ms）。
- **缺的：** 播放 WAV 的音频写入者、动作执行器、`showd`、`from_notes`，以及全部的契约和测试夹具。
