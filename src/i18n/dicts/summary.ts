/**
 * 汇总页词典：多 Agent 合计卡片、额度监控区、模型排行的文案。
 */

export const summary = {
  "summary.totalCost": "合计花费",
  "summary.sources": "{count} 来源",
  "summary.totalTokens": "合计 Token",
  "summary.costDist": "花费分布",
  "summary.quotaMonitor": "额度监控",
  "summary.resetDate": "重置 {date}",
  "summary.noToken": "未登录 Coding Plan",
  "summary.quotaFailed": "额度获取失败",
  "summary.noData": "暂无数据",
  "summary.loadFailed": "数据获取失败",
  "summary.notLoggedIn": "未登录",
  "summary.noQuotaData": "暂无额度数据",
  // OpenCode Go 额度口径标记：官方 API 返回的是权威值；本地库是按常量推算的
  // 估算值，两者不可混同（徽章上必须区分，否则用户会误以为都是官方额度）
  "summary.opencodeOfficial": "官方",
  "summary.opencodeEstimated": "本地估算",
};
