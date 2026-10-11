# 模型、供应商与工作区

LutriUI 将模型契约、工作区和供应商传输分开。新增模型时，先定义它能接收的输入和参数，再配置路由与界面。API Key、HTTP 请求和结果图片处理由 Rust 后端负责。

## 模块边界

| 位置                        | 职责                                                                                                |
| --------------------------- | --------------------------------------------------------------------------------------------------- |
| `shared/model-catalog.json` | 前后端共用的模型目录：家族、版本、供应商模型编号、参数规则、参考图上限、蒙版能力和文档来源          |
| `src/models/`               | 参数投影、模型校验和请求编译；FLUX 协议位于 `flux/protocol.ts`                                      |
| `src/workspaces/`           | 按家族注册的参数侧栏与工作区；通用控件和素材列表位于 `shared/`                                      |
| `src/app/useWorkspace.ts`   | 任务草稿、模型切换、撤销、偏好和持久化                                                                  |
| `src/app/generation.ts`     | 输入处理、生成快照、结果和历史材料组装                                                              |
| `src/components/`           | 画布、历史、设置、结果展示等通用交互                                                                |
| `src-tauri/src/models.rs`   | 读取同一模型目录，校验每个付费请求的路由、字段和输入约束                                            |
| `src-tauri/src/provider/`   | 各供应商的原生请求、鉴权、轮询和结果处理；凭据与共享传输分别位于 `credentials.rs`、`transport.rs` |

`Draft` 保存规范模型编号 `modelId`，供应商原生编号放在路由目录中。`routeSettings` 按模型和供应商保存参数；切回时恢复原设置，同家族的新路由会投影共享参数，不同家族首次使用其新建默认值，参数变化显示具体调整摘要。发送时只使用当前路由白名单中的字段。切换到不支持蒙版的路由时，保留蒙版并阻止提交，供用户切回或移除。供应商不会自动切换。

草稿格式为 `Draft.schema = 4`，会话格式为 `WorkspaceSession.schema = 2`，图库格式为 `PRAGMA user_version = 3`。`tasks.create` 和 `tasks.edit` 保存两份任务内容，撤销栈在当前会话中分别管理，`familyRoutes` 记忆每个家族最近使用的模型与供应商。模型选择不再切换整份草稿，结果也按生成/编辑任务归属。会话写入图库 SQLite 的 `workspace` 表；不迁移旧格式。损坏内容继续保留，直到用户明确新建方案。

## 已接入的组合

| 系列          | 版本            | 供应商                          |
| ------------- | --------------- | ------------------------------- |
| FLUX.3 Image  | FLUX.3 Image    | OpenRouter、BFL、Comfy、Runware |
| GPT Image 2.5 | Flare、Sunburst | OpenRouter、Comfy、Runware      |
| Qwen Image    | 3.0、3.0 Pro    | OpenRouter、Comfy、Runware      |
| Qwen Image    | 2.1 Pro、2.1 Turbo | QwenCloud；2.1 Pro 另有 Runware |
| Gemini Image  | Nano Banana 2.1 | OpenRouter、Comfy、Runware、Google |
| Gemini Image  | Nano Banana 2、Pro | OpenRouter、Comfy、Runware、Google |
| Seedream      | 5.0 Pro、Lite、Flash | OpenRouter、Comfy、Runware、火山方舟、BytePlus |
| Grok Imagine 2 | Image 2.0 | OpenRouter、Runware、Comfy、Grok 官方（xAI） |

目录于 2026-10-06 根据公开 API 资料核对，2026-10-07 补充核对 Nano Banana 2.1 的 OpenRouter、Google、Comfy 与 Runware 路由，2026-10-09 补充核对 Qwen Image 2.1 的 QwenCloud 与 Runware 路由。账户权限、价格与供应商后续变更仍以供应商实际响应为准。

## 工作区与 API 差异

FLUX 工作区保留参考图、来源区域和输出区域。BFL、Comfy 与 OpenRouter 将区域协议附在提示词中；Runware 使用 `settings.boundingBoxes`，单独发送不含区域 JSON 的指令。Runware 的 FLUX 文生图需要至少一个放置区域；指定分辨率和比例时，适配器与画布预估都读取目录里的 `fluxDimensions`，有参考图且比例为 `auto` 时使用分辨率预设。四个供应商的 FLUX 内容审核容忍度均限制为 0–4，0 最严格。Runware 还开放 PNG/JPEG/WebP 与 20–99 的压缩质量；PNG 不发送压缩质量。[BFL 文档](https://docs.bfl.ai/flux_3/flux3_image_overview)、[Runware 文档](https://runware.ai/docs/models/bfl-flux-3-image)。

GPT Image 工作区使用主图、参考图列表、质量、背景和输出格式。Comfy 与 Runware 支持自定义尺寸和蒙版；OpenRouter 当前路由使用宽高比，暂未开放蒙版和任意尺寸。内部蒙版使用透明 PNG，完全透明区域用于编辑，尺寸与第一张参考图一致；Runware 适配器转换为白色编辑、黑色保留的蒙版。参考图与蒙版按同一比例缩放。尺寸、透明背景与 JPEG 的冲突在提交前校验。[OpenAI 文档](https://developers.openai.com/api/docs/guides/image-generation)、[Comfy schema](https://docs.comfy.org/router-schemas/openai/gpt-image-2.5-flare.json)、[Runware 文档](https://runware.ai/docs/models/openai-gpt-image-2-5-flare)。

Qwen 工作区使用有序参考图和文字指令。3.0 与 3.0 Pro 走 OpenRouter、Comfy 和 Runware。OpenRouter 开放分辨率、宽高比、种子与张数，参考图上限为 4；Comfy 和 Runware 使用像素尺寸、负面提示词和扩写控制，参考图上限为 3。像素路由的最小面积、最大面积和长宽比写在目录的 `minPixels`、`maxPixels`、`maxAspect` 中，前后端读取同一组数字。Comfy 将尺寸编码为 `宽*高`；选择自动尺寸时省略该字段，并对 Pro 显示费用区间。Runware 使用独立宽高字段，并开放输出格式与压缩质量。有参考图时只支持 `direct` 扩写，关闭扩写时省略扩写方式。[OpenRouter 路由](https://openrouter.ai/api/v1/images/models/qwen/qwen-image-3/endpoints)、[Comfy schema](https://docs.comfy.org/router-schemas/qwen/qwen-image-3.0.json)、[Runware 文档](https://runware.ai/docs/models/alibaba-qwen-image-3-0)。

2.1 系列的规范编号是 `qwen-image-2.1-pro` 与 `qwen-image-2.1-turbo`。QwenCloud 同步接口为 `POST https://maas.qwencloudapi.com/api/v1/services/aigc/multimodal-generation/generation`，使用 `DASHSCOPE_API_KEY` 做 Bearer 鉴权；按量密钥走该地址，Token Plan 密钥不能混用。请求把参考图放在文字之前，编辑主图仍在第一位。尺寸使用 `宽*高`，省略时由模型决定；面积为 262,144–4,194,304 像素，比例至多 8:1。两个版本都支持最多 10 张参考图和 1–6 张输出，透明通道由提示词决定，不另发背景字段。Pro 支持 `direct` / `agent` 扩写和思考；思考只在扩写开启时发送，`agent` 不能用于有参考图的请求，也不支持负面提示词。Turbo 只保留扩写开关、水印、种子和最多 500 字的负面提示词，不发送扩写方式或思考。Runware 的 Pro 编号是 `alibaba:qwen-image@2.1-pro`，宽高各边为 192–4096，并开放输出格式与压缩质量；思考写入 `settings.thinking`。OpenRouter 与 Comfy Router 目前没有 2.1 路由。QwenCloud 尚无已确认的免费密钥检查接口。[QwenCloud 同步接口](https://docs.qwencloud.com/api-reference/image-generation/qwen-text-to-image)、[QwenCloud 编辑](https://docs.qwencloud.com/api-reference/image-generation/qwen-image-editing)、[Runware 2.1 Pro](https://runware.ai/docs/models/alibaba-qwen-image-2-1-pro)。

Grok Imagine 2 的规范编号为 `grok-imagine-image-2.0`，四条路由均支持 1K/2K 与低/中质量，不开放种子、蒙版或 FLUX 区域。OpenRouter 使用 `x-ai/grok-imagine-image-2.0`，有序参考图最多 3 张；Runware 使用 `xai:grok-imagine@image-2.0`，同样最多 3 张，并开放 PNG/JPEG/WebP 与压缩质量。Runware 的质量映射到 `settings.quality`，固定比例按目录内独立的 26 档尺寸表发送宽高；自动比例仅在有参考图时可用，发送分辨率预设并省略宽高。[OpenRouter 路由](https://openrouter.ai/api/v1/images/models/x-ai/grok-imagine-image-2.0/endpoints)、[Runware 文档](https://runware.ai/docs/models/xai-grok-imagine-image-2-0)。

Grok 官方使用 `XAI_API_KEY` 或系统凭据中的手动密钥，通过 Bearer 鉴权；连接检查读取 `/v1/models`。无参考图时调用 `/v1/images/generations`，有参考图时调用 `/v1/images/edits`，均发送 JSON；单图使用 `image` 对象，多图使用有序 `images` 数组，最多 5 张，主图仍位于第一位。官方另外开放自动质量、21:9 与 5:2 比例；自动质量当前文生图为 low、编辑为 medium。分辨率转换为小写 `1k/2k`，每次固定请求一张，批量由应用批次管理。结果兼容 URL 与 base64，保留审核拒绝原因；仅官方直连将实际 `cost_in_usd_ticks` 换算为美元，不推测缺失成本。[官方生成](https://docs.x.ai/developers/model-capabilities/images/generation)、[官方多图编辑](https://docs.x.ai/developers/model-capabilities/images/multi-image-editing)。

Comfy 的 `xai/grok-imagine-image-2.0` Router schema 当前只接受文字，参考图上限设为 0，编辑任务在提交前被阻止并提示切换供应商，已有素材继续保留。该模型使用同步 `POST /v2/models/xai/grok-imagine-image-2.0`，携带幂等键并直接读取结果，不进入其他 Comfy 模型的任务轮询；模型字段明确设为 2.0，避免 schema 的旧默认模型。公开 schema 虽列出 high，但其说明仅确认 2.0 的 low/medium 档，因此不开放 high；同样不发送官方独有的 auto 质量。实际 Comfy Credits 仍只读取响应头，不将上游美元用量当作 Credits。上述能力于 2026-10-07 核对。[Comfy schema](https://docs.comfy.org/router-schemas/xai/grok-imagine-image-2.0.json)。

## 任务与参考素材

Gemini Image 使用有序参考图、分辨率和宽高比，三个版本均至多 14 张参考图，不开放蒙版。Nano Banana 2.1 的规范编号为 `gemini-nano-banana-2.1`，OpenRouter 编号为 `google/gemini-nano-banana-2.1`；四个路由均开放 1K/2K/4K，不支持 512；OpenRouter、Google、Runware 开放 14 种固定比例，Comfy 另开放 9:21。除 OpenRouter 外均可选自动比例。OpenRouter 单次请求固定一张，多张生成由应用批次管理。默认模型仍为 Nano Banana 2，既有草稿和各路由参数继续保留。[Nano Banana 2.1](https://ai.google.dev/gemini-api/docs/models/gemini-nano-banana-2.1)、[OpenRouter 路由](https://openrouter.ai/api/v1/images/models/google/gemini-nano-banana-2.1/endpoints)。

Nano Banana 2.1 的 Comfy 编号为 `vertexai/gemini-nano-banana-2.1`，Runware 编号为 `google:nano-banana@2.1`。两者开放 minimal/medium/high 思考级别，默认 minimal；Comfy 另可返回思考摘要与文字说明，Runware 可分别开启网页与图片搜索。Comfy 的公开路由 schema 未列出搜索工具结构，暂不开放该控件。Runware 使用 2.1 独立的 42 档尺寸表，例如 4K 的 8:1 为 11712×1408，不复用 Nano Banana 2 的全景尺寸；种子范围为 0–2147483647。[Comfy schema](https://docs.comfy.org/router-schemas/vertexai/gemini-nano-banana-2.1.json)、[Runware 模型文档](https://runware.ai/docs/models/google-nano-banana-2-1)。

Google 官方路由使用 `x-goog-api-key` 请求头与 `generateContent`，输出配置为 `generationConfig.responseFormat.image`，不发送 Vertex AI 专用的输出格式字段。Comfy 使用 Vertex AI 原生 `generationConfig.imageConfig`，开放 1K/2K/4K 与 PNG/JPEG。结果按内容字段提取，兼容 inlineData 与 fileData，思考阶段的图片不计入最终结果。Google 支持选择是否返回文字说明与思考摘要，逐图保存这些信息。Runware 使用各模型独立的官方尺寸表；自动比例需要参考图并发送分辨率预设，其他比例发送精确宽高，另外开放种子与输出压缩。Google 的 Nano Banana 2.1 可选 minimal/medium/high（默认 medium），Nano Banana 2 可选 minimal/high（默认 minimal），Pro 不提供级别控件。三个模型均可启用网页搜索，2.1 与 2 另可启用图片搜索；默认关闭。结果逐张保存来源页面链接、搜索词、搜索建议 HTML、文字与摘要，图库独立保留元数据，删除生成记录后仍可查看。来源链接只允许 HTTP(S)，通过系统浏览器打开；搜索建议保留样式，在禁止脚本的 iframe 中显示，不进入应用 DOM。当前编辑仍以有序参考图和单次指令发起，不维持供应商多轮会话。[Google API](https://ai.google.dev/gemini-api/docs/generate-content/image-generation)、[Comfy schema](https://docs.comfy.org/router-schemas/vertexai/gemini-3.1-flash-image.json)、[Runware](https://runware.ai/docs/models/google-nano-banana-2)。

Seedream 支持 5.0 Pro、Lite 与 Flash，不开放独立蒙版或图层拆分。OpenRouter 三个版本均至多 14 张参考图；Pro/Flash 开放 1K/2K，Lite 开放 2K/4K。Comfy 和官方路由使用原生图像请求，Pro/Flash 至多 10 张参考图，Lite 至多 14 张；支持分辨率预设或自定义像素尺寸、PNG/JPEG 与水印。预设模式的比例由提示词描述，自定义尺寸按版本校验面积。Lite 的 Comfy 路由开放 2K/3K，官方路由的尺寸差异见下文。Runware Pro/Flash 使用自定义宽高，Lite 使用官方 2K/3K 尺寸表，不发送未开放的种子；输出支持 PNG/JPEG/WebP。Comfy 与官方 Lite 禁用连续出图，Runware Lite 将 `maxSequentialImages` 固定为 1，由应用批次管理张数。[Comfy schema](https://docs.comfy.org/router-schemas/byteplus/seedream-5-0-pro-260628.json)、[Runware Pro](https://runware.ai/docs/models/bytedance-seedream-5-0-pro)、[Runware Lite](https://runware.ai/docs/models/bytedance-seedream-5-0-lite)。

Seedream 官方分为火山方舟（国内）与 BytePlus ModelArk（国际），分别使用北京和东南亚 API 地址与独立密钥。国内 Pro/Flash 模型编号以 `doubao-` 开头，国际以 `dola-` 开头；Lite 分别为 `doubao-seedream-5-0-260128` 与 `seedream-5-0-260128`，不能混用。官方 Pro/Flash 支持 1K/1.5K/2K，Lite 支持 2K/3K/4K；官方自定义尺寸的 Pro/Flash 面积上限为 4,624,220px，Lite 为 16,777,216px，分别按官方规则校验，不沿用 Comfy 的限制。官方请求同步返回，付费请求不自动重试。Google 默认环境变量为 `GEMINI_API_KEY`，火山方舟为 `ARK_API_KEY`，BytePlus 为 `BYTEPLUS_API_KEY`，均支持自定义变量名或系统凭据中的手动密钥。Google 连接检查读取模型目录；火山方舟与 BytePlus 尚无已确认的免费密钥检查接口，保存密钥不承诺模型权限已验证。[火山方舟 API](https://docs.volcengine.com/docs/ark/image-generation-api?lang=zh)、[BytePlus API](https://docs.byteplus.com/en/docs/modelark/image-generation-api)、[BytePlus 模型说明](https://docs.byteplus.com/de/docs/modelark/seedream-5-0-pro)。新增家族没有 Comfy 预估价格表，界面不显示预估值；实际扣费仍读取 Comfy 响应头。

任务模式区分生成新画面与编辑图片。编辑主图始终排在请求的第一位，供自动比例、GPT 蒙版和结果对照使用。指定新主图时，程序同步重排素材并重映射提示词中的精确图片标签；已有蒙版时阻止更换主图，避免把蒙版作用于另一张图片。

参考素材可指定风格、主体、构图或自定义用途。`models/referenceInstructions.ts` 将用途和任务模式编译成生成指令，供应商请求不增加未支持的角色字段。历史配方保存 `intent`、素材用途、用途说明和路由参数。`baseId` 标记编辑主图；读入草稿时若顺序与主图不一致，会把主图排回第一位，并同步精确图片标签。

FLUX 主图等比显示。移除区域保留虚线标记。来源标记的显示与拖动计入主图留白，发送时仍使用各自独立的来源与输出坐标。构图画布尺寸不承诺 API 返回的精确像素尺寸。

偏好设置将 API 连接、新建默认值、外观和存储分开。每个家族可保存默认模型、供应商及对应参数，仅在新建方案时使用。设置标题、关闭按钮与历史返回入口保持可见。动画只用于状态转换与任务活动提示，遵循减少动态效果的系统偏好。

## 生成与存储

普通生成以候选网格为中央区域，按批次展示并逐张更新。编辑和 FLUX 区域构图使用画布，编辑页保留可折叠的近期生成面板。`regionsEnabled` 同时决定区域控件、生成页画布和请求编译；关闭区域构图会保留 `recipe.boxes`，实际请求与生成记录的区域字段为空。Runware 无参考图生成必须开启区域构图。生成张数属于草稿的 `repeatCount`，不作为供应商参数发送，导航和重启后保留。

生成先捕获当前草稿快照，处理输入，再提交一次付费请求。OpenRouter 同步返回结果；BFL、Comfy 和 Runware 提交后读取任务状态。Comfy 发送幂等键并遵循 `Retry-After`；Runware 以任务 UUID 轮询，并按图片标识去重。网络失败不会自动重新生成。不同提交与同一批次内的各张请求均并发执行，不等待已有生图任务完成；允许相同方案连续提交。生成张数表示独立请求次数，每次底层请求固定生成一张图片；同次提交作为一条独立任务和历史记录，汇总全部结果、逐张种子和状态；支持种子的路由在随机模式下为每张图片生成种子并保存到历史。保存提交记录期间的短暂提交锁防止意外连击。批次按结果到达顺序逐张保存结果，部分失败时保留成功图片。各张图片使用独立 `requestId`，通过 `historyId` 更新同一条历史；提示词、参考图和蒙版只保存一份。重启后仅恢复未开始的批次，已中断批次不会重新提交。

`generation-progress` 事件按 `requestId` 关联任务，显示准备、提交、等待、供应商已报告的生成阶段、下载和保存。事件通过 Rust 任务局部回调传递，前端为每张请求独立监听并在结束时释放监听器。应用不推测未返回的百分比。等待期间可以继续操作其他工作区；关闭应用前提示远程任务可能继续计费，用户可以继续等待或关闭。

多个结果关联同一生成记录，逐张保存并发布到界面；后续结果到达时保留当前图片选择。图库元数据、历史记录、标签、生成去重键和工作区草稿保存在 `<根目录>/library.sqlite3`（WAL 模式，`rusqlite` 直连，`library_sql.rs` 为全部查询的唯一来源），原图保留在 `gallery/images/<资产 UUID>/original.<ext>`，384px PNG 缩略图作为 BLOB 存入库中，由 `lutri-thumb` 自定义协议按资产 ID 提供；原图经 asset 协议按已有图片目录作用域提供，凭据、草稿与元数据不在协议作用域内。本机、剪贴板及 URL 导入也先入库。生成记录只持有输出图片 ID，并在 `images/<记录 ID>/` 独立保存输入与蒙版快照。图片在任务中的用途和主图角色属于工作副本。历史配方保存模型、供应商、参数、区域、参考图顺序与压缩设置。Comfy Credits 单独保留，不当作美元展示。保存失败时，结果仍可使用，并可重新保存而不触发生图。

图库根目录默认为 `app_data_dir/workbench`，用户可在设置的存储页选择其他目录；所选规范绝对路径原子保存在 `app_data_dir/library-location.json`（不在迁移目标内）。`storage.rs` 的迁移在存储门互斥下执行：数据库不直接复制文件，而是用 `rusqlite::backup::Backup` 写入目标 `library.sqlite3` 并运行 `quick_check` 校验，WAL/SHM 旁文件不参与复制；其余文件整树复制并逐块校验字节，拒绝符号链接、重解析点、非空目录以及源目录的祖先或后代目标；复制完成并打开新存储、注册图片资产作用域后才提交位置并切换活动根，源目录始终保留。凭据、密钥来源和前端偏好不随根目录迁移。

图库查询始终在数据库中分页（每页至多 120 张），支持搜索、来源、模型、标签、图板、可用状态和日期区间过滤，以及最新、最早、名称和大小排序；前端只持有已加载页，不会一次取回整库。扁平图板可创建、重命名、删除；图片可加入、移出或在图板之间移动。图库不再使用收藏标记：打开旧库时只补齐图板表并删除 `favorite` 列，不把星标迁入任何图板。MCP `gallery_query` 用 `boardId` 筛选，返回 `boardIds` 与全部图板；`gallery_board` 负责图板 CRUD 与成员变更。工作区选图复用同一组件，按选择顺序读入独立工作副本，检查已用素材和剩余名额。详情页提供名称、图板和标签编辑，元数据修改在同一事务内完成；批量操作按资产返回逐项成败。生成结果网格与图库资产提供「用这张去 Edit」：切换到编辑任务、设为主图、清空上一轮编辑指令，并按现有交接规则决定是否沿用参考图；不把 Create 的蒙版或区域拷到新主图上。所有图库操作只接受资产 ID，后端解析 UUID 对应的受管目录；删除先持久化删除标记，再删除文件和数据行，中断后可在重启时继续。生成输出使用记录 ID 与批次结果序号去重，删除资产后保留内部去重标记与有序关联，防止下一次批次保存恢复已删图片。删除图库图像不删除生成参数记录，记录中的对应结果显示已删除；删除生成记录不删除图库文件。设置页提供登记数量、缺失原图、待删除数量、原图与数据库字节数和上次检查时间，以及文件检查、缩略图重建和缺失记录清理（需确认）。新存储不读取或迁移旧 `history/`、`gallery/index.json` 与 `session.json`；`LibraryStore` 是纯服务层，与传输无关，MCP 原生图库/任务工具与 Tauri 命令经 `library_access.rs` 的同一迁移门访问它。

所有工作区共用「参数栏 → 参考素材栏 → 画布或生成结果」的布局。蒙版在原图像素空间绘制，画笔清除 alpha、橡皮恢复不透明像素；每次笔划结束保存一张 PNG，并作为一次撤销操作。矩形和导入蒙版可与笔划叠加。

Comfy 预估独立保存在 `shared/comfy-pricing.json`，记录核验日期与官方来源。FLUX 使用分辨率价格，Qwen 使用输出张数、Pro 面积档位和参考图数量，GPT 使用质量和尺寸的官方预估区间并估算输入费用。预估不作为扣费凭据。实际 Credits 从提交和结果轮询响应的 `X-Comfy-Credits-Used` 读取，后续响应缺失时保留已有值，不累加重复轮询返回的费用。该响应头只对部分模型和成功响应提供；上游响应体的 `cost` 或 `credits` 不代表 Comfy 扣费。没有实际扣费信息时，结果及历史隐藏成本项。[Comfy 计费说明](https://docs.comfy.org/development/comfy-router/billing)。

会话 `WorkspaceSession.schema = 2` 只保存生成与编辑两份 `Draft`。每份草稿用 `familyRoutes` 记住六个家族最近使用的模型与供应商。无法识别的版本或损坏内容暂停自动保存，保留原文件，不迁移旧 schema。新建方案可恢复保存。

撤销记录、结果选择和本会话最近 20 次尝试按任务意图隔离。结果原图来自请求快照，后台完成只更新发起任务的结果。任务切换属于导航，替换编辑主图属于可撤销修改。重启恢复草稿，完整结果通过本地历史查看。

`useGenerationTasks` 保存不可变任务快照并并发执行：先持久化 `queued` 历史与原始素材，再保存 `running` 状态，最后调用供应商。每张请求使用独立请求 ID，同一批次共享历史 ID；完成与失败更新同一条记录。可停止尚未发送的批次请求，已经发出的请求继续接收并保存结果。仅同一历史记录的本地写入串行，供应商调用不等待其他请求的结果。记录未落盘时不调用供应商，失败不会自动重发。供应商任务 ID 和实际进度在 Rust 历史存储中更新。启动时将上次的 `running` 标记为 `interrupted`，只恢复 `queued` 请求；中断请求可能仍在供应商执行。恢复的未开始批次同样并发执行。React StrictMode 的过期恢复过程不启动任务，避免恢复两次。

桌面应用标识与系统凭据命名空间使用 `app.lutriui.desktop`，前端设置键使用 `lutriui-preferences-v2`，导出文件名前缀使用 `lutriui-`。改名后使用新的本地数据目录和凭据，不读取或迁移旧标识下的数据。

## MCP 智能体接入

应用内置进程内 MCP 服务（`src-tauri/src/mcp.rs`），使用官方 `rmcp` SDK 的 Streamable HTTP 传输，只绑定 `127.0.0.1`（默认端口 39631，1024–65535 可配），不做第二个 JSON-RPC 实现或独立守护进程。HTTP 边界在每个 MCP 方法分发前校验：Bearer 令牌（常量时间比较，只存系统凭据 `app.lutriui.mcp`/`access-token`，不进入配置、数据库或日志）、Host 必须匹配 `127.0.0.1:<port>` 或 `localhost:<port>`、存在的 Origin 必须是合法 UTF-8 的本地回环来源，未授权 401、越界 403，不开放 CORS。工具定义固定来自 `shared/mcp-tools.json` 的 12 个工具；参数在分发前用 jsonschema（Draft 2020-12，启用 format 校验）按已发布 schema 深校验，错误只回 JSON 指针路径，不回显数据体。

配置 `{enabled, port}` 原子保存在 `app_data_dir/mcp.json`；`configure`/`start`/`rotate`/`stop` 由生命周期互斥串行：先准备令牌、监听套接字和持久化配置，再原子替换活动认证与服务器，失败保留旧端点；同端口已运行直接返回；禁用先写盘再停止；监听器崩溃反映为 `running:false` 加状态事件。轮换令牌先写凭据库再替换活动认证，旧 Bearer 下一请求即失效。用户启用服务时若凭据库没有令牌，新令牌只随该次 `configure` 结果返回并在设置页直接显示；启动失败则留到下一次成功的启用再返回。状态事件不携带令牌。敏感命令只接受 `main` 窗口。

工作区/任务操作通过有界桥接到达活动 WebView：Rust 保存至多 32 条 pending（30 秒超时），向主窗口发送 `mcp-request`（requestId、instanceId、operation、args、deadlineMs）；前端先入队再在分发前 `mcp_bridge_claim`，已认领工作超时或断开返回 `OUTCOME_UNKNOWN`（附 idempotencyKey/expectedVersion 提示），未认领返回 `UI_NOT_READY`；不同实例注册会取消先前 pending，同实例重复注册是空操作；分发 future 被丢弃时由 drop guard 移除 pending 项，不会耗尽容量。工作区与生成恢复完成后前端才注册，卸载或失焦只移除匹配实例。

工具分为两层：图库、图板与任务读取/导入在 Rust 侧直接用 `LibraryStore`（经 `library_access.rs` 的迁移门与阻塞池）；`task_get` 投影历史记录的公开字段，空结果列表不再查图库。工作区操作走 React 桥的共享动作层（`src/app/applicationActions.ts`、`workspaceActions.ts`、`useMcpBridge.ts`）——UI 的生成按钮与 MCP `task_submit` 走同一条 `prepareGeneration` → 校验 → 持久化 → 提交路径，提交的是 `singleImageDraft` 不可变快照。工作区版本是 `${instanceId}:${revision}` 的内存纪元（重启即新纪元），每次 `replace` 同步递增；`expectedVersion` 不匹配返回 `VERSION_CONFLICT`。已声明动作在资源 await 前后检查活动性（disposed/实例/截止时间），过期抛出 `REQUEST_EXPIRED`，不会写入。`workspace_patch` 的参数校验严格按目标路由 `fieldsFor` 推导：未知键、错误类型、越界、`count≠1` 都拒绝且不提交；成功补丁在 `flushSync` 内恰好一次 `commit(..., true)`，可整体撤销。

删除仍被旧提示词引用的参考图时，补丁仍然成功；若未同时提供面向最终引用顺序的新提示词，响应通过 `warnings` 返回 `PROMPT_REFERENCES_REMOVED`、原始图片标签和已移除引用，要求智能体检查返回的提示词并在提交前修正。区域以 `uid` 作为身份和去重依据，省略即新建；`id` 仍须作为提示词标签保持唯一。`gallery_query` 省略 `query` 等同于空对象，并返回当前图板列表。`gallery_board` 用 `op` 区分 list/create/rename/delete/add/remove/move。`task_get` 保留结果资产 ID 与顺序，以 `available`、`missing`、`pendingDelete`、`deleted` 区分结果可用性，不改写任务成功状态。

`task_submit` 的幂等账本 `mcp_submissions`（`library_sql.rs` 的 MCP_SCHEMA）与初始历史记录在同一事务写入、独立于历史删除而存活：相同 `idempotencyKey`+`workspaceVersion` 返回原 `taskId` 且不重复提交，版本冲突拒绝 `IDEMPOTENCY_CONFLICT`，前端同 key 并发共享同一 promise。图片始终以资产 ID 或工作区 uid 引用，无文件路径访问；供应商凭据与应用设置不通过工具暴露；提交即可能扣费，没有额外确认环节。

## 新增模型或供应商

1. 在 `shared/model-catalog.json` 中添加模型版本、已确认的供应商路由和能力规则。模型编号使用应用内规范编号，供应商编号放入各自路由。
2. 为新的参数扩展 TypeScript `GenerateParams` 和 Rust `GenerateParams`。在模型模块中添加组合约束，在供应商适配器中添加原生字段映射与响应处理。
3. 新增家族时，扩展 `FamilyId`，提供独立工作区，并在 `src/workspaces/index.tsx` 中注册。只有操作语义相同的部分才复用通用控件。
4. 新增供应商时，添加凭据来源、状态与连接检查，在 `provider::dispatch` 中注册适配器，并将相应路由加入目录。
5. 维护模型契约和关键行为测试，不用整页文案、图标路径或下拉选项快照验证界面。运行 `bun run test`、`bun run build` 和 `cargo test --manifest-path src-tauri/Cargo.toml`；桌面交互另行实测。

自动验证覆盖请求字段投影、坐标与参考图身份、蒙版保护、路径重定位、HTML 安全隔离、SQLite 资产持久化、删除与重试一致性、图库迁移以及本地 HTTP 模拟。前端测试不依赖指定语言或整页交互模拟。`src-tauri/tests/live_nano_banana.rs` 与 `src-tauri/tests/live_creative_workflow.rs` 保留为显式付费验证，默认 `#[ignore]`，不会随普通测试执行。界面文案以 `scripts/emit-messages.ts` 为源，写入 `messages/` 后再执行 `bun run i18n:compile`。供应商名单和默认环境变量名从目录的 `providers` 读取，不再在凭据模块里另写一份。

工作台采用「参数栏 → 参考素材栏 → 画布／生成结果」布局。素材栏独立于各模型的画布与结果视图，切换视图时保留素材操作位置；左侧两栏分别滚动并保存宽度。素材栏右边界支持拖拽和方向键调节，窄窗口优先为右侧工作区保留 320px；不足时允许横向滚动。
