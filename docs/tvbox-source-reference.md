# TVBox 配置接口参考

这些地址仅作为用户可选的第三方配置参考，不会在应用启动时自动请求，也不会被当作已验证的播放源。使用前请确认来源的合法性、稳定性和隐私政策；应用只应通过后续的“添加外部目录”流程显式导入。

| 名称 | 配置地址 |
| --- | --- |
| 网络接口 | `https://8815.kstore.vip/tvbox/wmz` |
| 真心 | `https://tvbox.catvod.com/FongMi.json` |
| 虎斑 | `http://hb.小虎斑.site:25252/仅供测试` |
| 饭太硬 | `https://www.饭太硬.cc/tv` |
| 肥猫 | `http://肥猫.net/tv` |
| vox | `http://rihou.cc:88/demo.php` |
| 小米 | `https://gh-proxy.org/https://raw.githubusercontent.com/ggrrttyyiii/CatVodSpider/refs/heads/main/json/demo.json` |
| 摸鱼儿 | `https://6800.kstore.vip/fish.json` |
| 讴歌 | `https://欧歌.v.nxog.top/m` |
| PG | `https://tvbox.catvod.com/jsm.json` |
| 多多 | `https://yydsys.top/duo` |
| 南风 | `https://gh-proxy.com/https://raw.githubusercontent.com/yoursmile66/TVBox/refs/heads/main/XC.json` |
| 王二小 | `https://9280.kstore.vip/newwex.json` |
| 东篱 | `https://16151.kstore.space` |
| 嗷呜 | `http://www.英格里希嗷呜.top/tv` |
| 潇洒 | `https://9877.kstore.space/single.json` |

## 元数据字段

兼容常见 TVBox VOD 字段：`vod_name`、`vod_pic`、`vod_year`、`vod_class`、`vod_score`、`vod_content`。导入后的媒体会映射到影视库标题、海报、年份、类型、评分和简介；`adult`、`isAdult`、`contentRating`、`vod_remarks` 以及文件名中的 18+ 标记会写入 `adult` / `contentRating` / `genres`，并在卡片与详情页显示 18+ 标签。
