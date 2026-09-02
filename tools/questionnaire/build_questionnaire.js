const fs = require('fs');
const path = require('path');
const { Document, Packer, Paragraph, TextRun, Table, TableRow, TableCell, HeadingLevel, AlignmentType, WidthType, BorderStyle, ShadingType, Footer, Header, PageNumber } = require('./.docx-tools/node_modules/docx');

const out = path.join(__dirname, 'TTV后续方向问题清单.docx');
const W = 9360;
const border = { style: BorderStyle.SINGLE, size: 1, color: 'D9E1F2' };
const borders = { top: border, bottom: border, left: border, right: border };
const children = [];
const docOptions = {
  styles: {
    default: { document: { run: { font: 'Microsoft YaHei', size: 22 } } },
    paragraphStyles: [
      { id: 'Heading1', name: 'Heading 1', basedOn: 'Normal', next: 'Normal', quickFormat: true, run: { font: 'Microsoft YaHei', size: 32, bold: true, color: '1F4E79' }, paragraph: { spacing: { before: 280, after: 160 }, outlineLevel: 0 } },
      { id: 'Heading2', name: 'Heading 2', basedOn: 'Normal', next: 'Normal', quickFormat: true, run: { font: 'Microsoft YaHei', size: 27, bold: true, color: '2F75B5' }, paragraph: { spacing: { before: 220, after: 120 }, outlineLevel: 1 } }
    ]
  },
  sections: [{
    properties: { page: { size: { width: 12240, height: 15840 }, margin: { top: 1200, right: 1440, bottom: 1200, left: 1440 } } },
    headers: { default: new Header({ children: [new Paragraph({ alignment: AlignmentType.RIGHT, children: [new TextRun({ text: 'TTV 项目需求确认问卷', size: 18, color: '808080' })] })] }) },
    footers: { default: new Footer({ children: [new Paragraph({ alignment: AlignmentType.CENTER, children: [new TextRun('第 '), new TextRun({ children: [PageNumber.CURRENT] }), new TextRun(' 页')] })] }) },
    children
  }]
};
const c = children;
const para = (text, opts = {}) => c.push(new Paragraph({ spacing: { after: 100 }, ...opts, children: [new TextRun({ text, ...(opts.run || {}) })] }));
const heading = (text, level = HeadingLevel.HEADING_1) => c.push(new Paragraph({ heading: level, children: [new TextRun(text)] }));
const note = text => c.push(new Paragraph({ shading: { fill: 'F3F6FA', type: ShadingType.CLEAR }, indent: { left: 180, right: 180 }, spacing: { before: 80, after: 120 }, children: [new TextRun({ text, italics: true, color: '4F4F4F' })] }));
let number = 1;
function question(title, ask, evidence, lines = 3) {
  c.push(new Table({ width: { size: W, type: WidthType.DXA }, columnWidths: [650, 8710], rows: [new TableRow({ children: [
    new TableCell({ width: { size: 650, type: WidthType.DXA }, borders, shading: { fill: 'D9EAF7', type: ShadingType.CLEAR }, margins: { top: 100, bottom: 100, left: 100, right: 100 }, children: [new Paragraph({ alignment: AlignmentType.CENTER, children: [new TextRun({ text: String(number++), bold: true, color: '1F4E79' })] })] }),
    new TableCell({ width: { size: 8710, type: WidthType.DXA }, borders, margins: { top: 100, bottom: 100, left: 140, right: 140 }, children: [
      new Paragraph({ spacing: { after: 50 }, children: [new TextRun({ text: title, bold: true, color: '1F4E79' })] }),
      new Paragraph({ spacing: { after: 50 }, children: [new TextRun(ask)] }),
      new Paragraph({ spacing: { after: 50 }, children: [new TextRun({ text: '已有依据 / 默认建议：', bold: true, color: '666666' }), new TextRun({ text: evidence, color: '666666' })] }),
      new Paragraph({ spacing: { after: 30 }, children: [new TextRun({ text: '你的回答：', bold: true, color: '2F75B5' })] }),
      ...Array.from({ length: lines }, () => new Paragraph({ spacing: { after: 20 }, children: [new TextRun('________________________________________________________________________________')] }))
    ] })
  ] })] }));
  para('');
}

c.push(new Paragraph({ alignment: AlignmentType.CENTER, spacing: { after: 100 }, children: [new TextRun({ text: 'TTV 后续方向问题清单', bold: true, size: 42, color: '1F4E79' })] }));
c.push(new Paragraph({ alignment: AlignmentType.CENTER, spacing: { after: 150 }, children: [new TextRun({ text: '基于 LumiPlayer 聊天记录与逆向报告整理', italics: true, size: 24, color: '666666' })] }));
para('版本：需求澄清稿 1.0    日期：2026-08-18', { alignment: AlignmentType.CENTER });
note('填写方式：直接在每题“你的回答”处填写；选择题可保留选项并标记。如果暂时不确定，填写“待定”并补充原因。文档区分“逆向已确认事实”和“产品需要你决定的方向”，后者不会擅自替你定案。');

heading('一、当前已确认基线');
para('逆向报告确认 LumiPlayer 是 Tauri 2.11.5 + Rust + WebView2 的桌面媒体客户端，采用 Rust 主进程、StreamHub 本地媒体服务、Lumi Cloud 远端服务三轨架构。');
para('当前 TTV 规划包含播放器、媒体库、云盘与媒体服务器聚合、元数据、字幕、播放增强、AI 搜索/推荐/Agent、插件和更新系统。');
para('建议第一阶段先完成 TTV v1.0：本地文件播放、基础媒体库、播放进度、字幕和可扩展 Provider 接口。云盘、AI、License 和高级增强不作为第一阶段阻塞项。');
note('主要待动态确认项：Provider sign 公式、license.dat 格式与密钥来源、完整性监视动作、更新替换流程。');

heading('二、产品目标与范围');
question('产品定位', 'TTV 首个可发布版本的定位是什么？', '[ ] 个人自用播放器  [ ] 开源 LumiPlayer 类似产品  [ ] 商业化桌面客户端  [ ] 内部工具  [ ] 其他：', 2);
question('目标平台', '首发平台和最低支持版本是什么？', '默认 Windows 10/11 x64；是否需要 macOS、Linux、ARM 或 Steam Deck？', 2);
question('版本边界', '本轮希望直接做到哪个版本范围？', '建议先锁定 v1.0 播放核心，再拆 v1.5/v2.0/v2.5/v3.0。', 3);
question('兼容目标', '与 LumiPlayer 的兼容程度到什么层级？', '[ ] 功能等价  [ ] UI 相似  [ ] 数据可迁移  [ ] 配置可迁移  [ ] 协议兼容  [ ] 仅实现同类能力', 2);
question('发布方式', '首个版本如何交付和更新？', '[ ] 单机压缩包  [ ] 安装程序  [ ] MSIX  [ ] GitHub Release  [ ] 自建更新服务器  [ ] 仅源码构建', 2);

heading('三、TTV v1.0 播放核心');
question('播放器内核', '是否确认采用 libmpv.dll 动态链接？', '推荐 Rust FFI 封装 libmpv，外置 mpv-2.dll 和依赖，便于升级排错。', 2);
question('渲染方式', '播放器采用哪种窗口方式？', '[ ] WebView2 内嵌  [ ] 独立播放窗口  [ ] Tauri child window  [ ] 先独立窗口后嵌入', 2);
question('视频能力', 'v1.0 必须支持哪些视频能力？', '[ ] 本地文件  [ ] HTTP 直链  [ ] HLS  [ ] DASH  [ ] HDR  [ ] 硬件解码  [ ] 倍速  [ ] 截图  [ ] 画中画', 3);
question('音频能力', '音轨和音频输出需要做到什么程度？', '[ ] 单音轨  [ ] 多音轨切换  [ ] 音量/增益  [ ] WASAPI 独占  [ ] 5.1/7.1  [ ] 音频延迟', 2);
question('字幕能力', '字幕在 v1.0 需要哪些功能？', '[ ] 外挂字幕  [ ] 内嵌字幕  [ ] 字幕切换  [ ] 字体/大小/位置  [ ] 字幕延迟  [ ] 在线下载  [ ] OpenSubtitles', 3);
question('播放状态', '播放记录需要保存哪些字段？', '建议：媒体 ID、位置、总时长、完成比例、最近播放时间、音轨、字幕轨、播放速度。', 3);
question('播放失败', '直链失效、403、超时、格式不支持时，期望怎样降级？', '建议：重试 → 刷新 Provider → HLS/转码代理 → 外部播放器 → 明确错误。', 3);
question('外部播放器', '是否保留 mpv、PotPlayer、VLC、MPC-HC/BE 等外部播放器唤起？', '逆向报告确认原产品有播放器探测逻辑；需要明确首版是否实现。', 2);

heading('四、媒体库与数据模型');
question('媒体库来源', 'v1.0 媒体库先支持哪些来源？', '[ ] 本地目录  [ ] NAS/SMB  [ ] WebDAV  [ ] Emby  [ ] Jellyfin  [ ] Plex  [ ] 云盘', 3);
question('扫描方式', '媒体库扫描采用自动监控还是手动扫描？', '[ ] 启动扫描  [ ] 定时扫描  [ ] 文件系统监听  [ ] 手动扫描  [ ] 增量扫描', 2);
question('媒体类型', '首版支持哪些媒体类型？', '[ ] 电影  [ ] 电视剧  [ ] 动漫  [ ] 纪录片  [ ] 演唱会  [ ] 综艺  [ ] 音乐  [ ] 直播', 2);
question('命名规则', '是否兼容 Jellyfin/Emby/TMDB 常见命名规则？', '建议支持 Movie (Year)、S01E01、分辨率、音轨、字幕语言等常见信息提取。', 3);
question('元数据来源', '元数据优先使用哪些服务？', '[ ] TMDB  [ ] MDBList  [ ] Emby/Jellyfin 自带  [ ] 本地 NFO  [ ] 自建服务  [ ] 不做自动刮削', 3);
question('数据库', 'TTV 使用独立 SQLite，还是必须兼容现有 StreamHub/lumi-store.db？', '建议使用自己的 schema，并提供导入/迁移工具，不直接复用逆向目标内部数据库。', 3);
question('数据迁移', '需要从哪些现有数据迁移？', '[ ] 播放历史  [ ] 媒体库  [ ] 账号  [ ] 云盘配置  [ ] 播放器设置  [ ] 不需要迁移', 3);

heading('五、Provider 与媒体源聚合');
question('Provider 范围', 'v1.0 要接哪些 Provider？请按优先级排序。', '已发现百度、阿里、夸克、115、天翼、光鸭、飞牛，以及 Emby/Jellyfin/Plex。', 4);
question('登录方式', '云盘登录是否必须二维码 OAuth？', '各家均有二维码登录/轮询；也可先支持已有账号 token 导入。', 3);
question('凭据存储', '云盘 token 和媒体服务器凭据采用什么存储策略？', '建议统一使用 Windows Credential Manager/DPAPI，禁止明文 token 落盘。', 3);
question('Provider 接口', 'Provider 是否统一抽象为可插拔接口？', '建议统一 login、refresh、list、search、resolve、headers、subtitle、capabilities。', 3);
question('直链解析', '解析失败时是否允许使用自建后端或代理？', '建议 endpoint 可配置、默认 HTTPS，并支持本地直连优先。', 2);
question('媒体服务器', 'Emby/Jellyfin/Plex 首版需要支持哪些操作？', '[ ] 登录/保存服务器  [ ] 浏览库  [ ] PlaybackInfo  [ ] 直链播放  [ ] 转码  [ ] 播放进度回写  [ ] 字幕', 3);

heading('六、UI 与交互');
question('首页结构', '首页首屏要展示哪些内容？', '建议：继续观看、最近添加、最近播放、收藏、媒体源入口、搜索。', 3);
question('导航结构', '采用侧边栏、顶部导航还是双栏布局？', '[ ] 侧边栏  [ ] 顶部导航  [ ] macOS 风格侧栏  [ ] 你提供截图后复刻', 2);
question('播放页', '播放页是否需要独立路由/独立窗口？', '请明确控制条、播放列表、字幕、音轨、清晰度等是否存在。', 3);
question('主题', '主题和视觉风格是什么？', '[ ] 深色影院  [ ] 浅色简洁  [ ] 跟随系统  [ ] 自定义品牌色：', 2);
question('参考资料', '你能提供哪些 UI 参考资料？', '[ ] LumiPlayer 截图  [ ] 操作录屏  [ ] 页面清单  [ ] TTV 草图  [ ] 暂无', 3);

heading('七、AI、增强与插件');
question('AI 范围', 'AI 搜索、推荐、Agent 在哪个版本实现？', '逆向报告显示 RAG 配置存在但未接线；建议放到 v2.0，不阻塞 v1.0。', 3);
question('AI 服务', 'AI 使用云端 API、局域网模型还是本地模型？', '[ ] OpenAI 兼容 API  [ ] Ollama  [ ] LM Studio  [ ] 自建服务  [ ] 暂不确定', 3);
question('视频增强', 'VapourSynth、RIFE、超分、锐化、HDR 是否属于首版范围？', '原产品包含 Python 3.12 + VapourSynth + RIFE；建议先实现可插拔管线。', 3);
question('插件体系', '是否需要第三方插件 API，还是只保留视频滤镜插件？', '当前只确认 VapourSynth 插件体系，未确认通用插件市场。', 3);

heading('八、账号、License 与安全');
question('账号体系', 'TTV 是否需要自建账号系统？', '[ ] 单机无账号  [ ] 本地账号  [ ] 云端账号  [ ] 邮箱验证  [ ] 社交登录', 2);
question('License 模式', '授权采用什么模式？', '[ ] 完全免费  [ ] 本地 License 文件  [ ] 在线激活  [ ] 订阅  [ ] 设备数限制  [ ] 永久授权', 3);
question('离线能力', '离线状态下允许使用哪些功能、持续多久？', '逆向目标是离线文件 + 在线激活混合模型；需要明确 TTV 策略。', 3);
question('设备绑定', '是否绑定设备？绑定哪些标识？', '建议使用可撤销设备记录，不设计成不可迁移的永久锁。', 3);
question('敏感数据', '哪些数据禁止上传或写入日志？', '建议明确 token、密码、设备指纹、媒体路径、播放历史、AI 对话的日志策略。', 3);

heading('九、更新、部署与运维');
question('更新策略', '更新是全量包、差分包还是运行时资源分包？', '原产品使用 version.json + SHA-256 + _up_ 运行时暂存目录。', 2);
question('更新服务器', '是否已有域名、对象存储、CDN 和版本清单服务？', '没有的话，首版可用静态 JSON + HTTPS 文件下载。', 3);
question('回滚', '更新失败时是否必须自动回滚？', '建议保留上一版本目录、校验包 hash、失败后恢复。', 2);
question('运行依赖', '是否允许依赖系统 Java 21？', '原 StreamHub 依赖 Temurin 21；也可内置 JRE 或改用 Rust/Go 服务。', 3);

heading('十、工程组织与验收');
question('仓库结构', '采用单仓库还是前后端分仓？', '[ ] Tauri 单仓库  [ ] Rust/前端/服务三目录  [ ] 独立仓库  [ ] 已有模板：', 2);
question('技术栈', '是否确认 Tauri + Rust + TypeScript/React？', '原前端更接近 vanilla JS；新项目建议 TypeScript + React/Svelte 降低维护成本。', 3);
question('测试标准', '首版必须通过哪些验收？', '[ ] 单元测试  [ ] 集成测试  [ ] 播放样本集  [ ] 断网测试  [ ] 4K/HDR  [ ] 长时间稳定性', 3);
question('性能目标', '请给出启动、扫描、播放和内存目标。', '填写冷启动秒数、首帧时间、4K CPU/GPU、空闲内存、库扫描速度。', 4);
question('第一批样本', '请提供用于验收的媒体和环境样本。', '建议：本地电影、电视剧目录、多音轨/字幕文件、HLS 地址、Emby/Jellyfin 测试环境或 mock 服务。', 4);

heading('十一、请优先回答的 10 个问题');
para('如果不想一次回答全部问题，先回答下面 10 个，足以启动 TTV v1.0 的工程设计：');
['产品定位和首发平台是什么？','是否确认 v1.0 采用 libmpv 动态链接？','v1.0 是否只做本地文件 + 基础媒体库？','首版是否必须支持 Emby/Jellyfin/Plex？优先级如何？','首版要接哪些云盘 Provider？','是否需要二维码登录，还是先支持 token 导入？','是否需要账号和 License？采用免费、订阅还是永久授权？','UI 是否有截图/录屏/品牌规范？','AI、RIFE、超分、RAG 放在哪个版本？','下一步直接生成项目骨架，还是先补技术设计文档？'].forEach((x, i) => para(`${i + 1}. [ ] ${x}`));

heading('十二、参考文件');
['聊天记录/LumiPlayer 逆向分析_ab187767d7b1.md','逆向报告/REVERSE_ENGINEERING_REPORT.md','逆向报告/architecture.md','逆向报告/api/backend_api.md','逆向报告/protocol/cloud_oauth.md','逆向报告/update_license.md'].forEach(x => para(x));

const doc = new Document({ ...docOptions, sections: [{ ...docOptions.sections[0], children }] });
Packer.toBuffer(doc).then(buf => fs.writeFileSync(out, buf));
