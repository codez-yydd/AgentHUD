//
//  TrendView.swift
//  agenthud
//
//  趋势图卡（可复用子视图）：Token / 花费切换的单图 + 峰值 / 日均摘要。
//  数据由父视图传入（[TrendPoint]，通常由 /usage 的 trend 字段经
//  Billing.remoteTrendToLocal 折算而来），本文件不再包含独立页面外壳
//  （原「趋势」Tab 已并入总览页的趋势卡，由 OverviewView 复用）。
//

import SwiftUI
import Charts

// MARK: - 趋势图卡（总览页趋势卡复用）

struct TrendChartCard: View {
    let points: [TrendPoint]
    @EnvironmentObject var settings: AppSettings

    @State private var metric: Metric = .tokens

    enum Metric: String, CaseIterable {
        case tokens, cost
    }

    private var color: Color {
        metric == .tokens ? Theme.accent : Theme.mint
    }

    private var values: [Double] {
        points.map { p in
            metric == .tokens ? Double(p.total_tokens) : costValue(p)
        }
    }

    private var totalText: String {
        if metric == .tokens {
            return Format.tokens(points.reduce(0) { $0 + $1.total_tokens })
        }
        return Format.cost(points.reduce(0) { $0 + costValue($1) }, settings.currency)
    }

    private var peakText: String {
        guard let peak = values.max() else { return "—" }
        return metric == .tokens ? Format.tokens(Int(peak)) : Format.cost(peak, settings.currency)
    }

    private var averageText: String {
        guard !values.isEmpty else { return "—" }
        let avg = values.reduce(0, +) / Double(values.count)
        return metric == .tokens ? Format.tokens(Int(avg)) : Format.cost(avg, settings.currency)
    }

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 8) {
                    CardHeader(title: "趋势",
                               icon: "chart.xyaxis.line",
                               tint: color,
                               detail: "\(points.count) 个时间桶")
                    metricSwitch
                }

                HStack(spacing: 12) {
                    HeroMetric(label: metric == .tokens ? "桶合计" : "总花费",
                               value: totalText, tint: color)
                    HeroMetric(label: "单桶峰值", value: peakText)
                    HeroMetric(label: "单桶均值", value: averageText)
                }

                chart
                    .frame(height: 172)
                    .padding(.top, 2)
            }
        }
    }

    /// 图表数据：用序号做数值 X 轴（分类轴在 30 个日期时会每点都画标签，
    /// 叠成一片）；轴标签再用序号映射回日期文本。
    private struct ChartDatum: Identifiable {
        let id: Int
        let index: Int
        let label: String
        let value: Double
    }

    private var chartData: [ChartDatum] {
        points.enumerated().map { index, p in
            ChartDatum(id: index,
                       index: index,
                       label: p.label,
                       value: metric == .tokens ? Double(p.total_tokens) : costValue(p))
        }
    }

    /// X 轴只保留最多 5 个刻度，且始终包含最新的一个。
    private var xTickIndices: [Int] {
        guard !points.isEmpty else { return [] }
        let step = max(1, Int(ceil(Double(points.count) / 5.0)))
        var indices: [Int] = []
        var index = points.count - 1
        while index >= 0 {
            indices.append(index)
            index -= step
        }
        return indices.reversed()
    }

    private var chart: some View {
        let data = chartData
        return Chart(data) { d in
            if data.count > 16 {
                // 长区间用面积 + 折线，避免 30 根细柱造成的锯齿感
                AreaMark(
                    x: .value("时间", d.index),
                    y: .value("数值", d.value)
                )
                .foregroundStyle(
                    LinearGradient(colors: [color.opacity(0.30), color.opacity(0.0)],
                                   startPoint: .top, endPoint: .bottom))
                .interpolationMethod(.monotone)

                LineMark(
                    x: .value("时间", d.index),
                    y: .value("数值", d.value)
                )
                .foregroundStyle(color)
                .lineStyle(StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
                .interpolationMethod(.monotone)
            } else {
                BarMark(
                    x: .value("时间", d.index),
                    y: .value("数值", d.value)
                )
                .foregroundStyle(
                    LinearGradient(colors: [color.opacity(0.92), color.opacity(0.42)],
                                   startPoint: .top, endPoint: .bottom))
                .cornerRadius(3)
            }
        }
        .chartXScale(range: .plotDimension(startPadding: 6, endPadding: 6))
        .chartXAxis {
            AxisMarks(values: xTickIndices) { value in
                AxisValueLabel {
                    if let index = value.as(Int.self),
                       data.indices.contains(index) {
                        Text(data[index].label)
                            .font(.system(size: 9))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 3)) { value in
                AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [3, 4]))
                    .foregroundStyle(Color.white.opacity(0.06))
                AxisValueLabel {
                    if let raw = value.as(Double.self) {
                        Text(axisLabel(raw))
                            .font(.system(size: 9))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
            }
        }
        .chartPlotStyle { plot in
            plot.background(Color.clear)
        }
    }

    private var metricSwitch: some View {
        HStack(spacing: 2) {
            ForEach(Metric.allCases, id: \.self) { m in
                Button {
                    withAnimation(.easeOut(duration: 0.18)) { metric = m }
                } label: {
                    Text(m == .tokens ? "Token" : "花费")
                        .font(.system(size: 10, weight: metric == m ? .semibold : .regular))
                        .foregroundStyle(metric == m ? color : Theme.textTertiary)
                        .padding(.horizontal, 9)
                        .padding(.vertical, 4)
                        .background {
                            if metric == m {
                                Capsule().fill(color.opacity(0.14))
                            }
                        }
                }
                .buttonStyle(.plain)
            }
        }
        .padding(2)
        .background(Capsule().fill(Color.white.opacity(0.045)))
    }

    private func costValue(_ p: TrendPoint) -> Double {
        settings.currency == .cny ? p.cost_cny : p.cost_usd
    }

    private func axisLabel(_ value: Double) -> String {
        if metric == .tokens {
            return Format.tokens(Int(value))
        }
        let symbol = settings.currency == .cny ? "¥" : "$"
        if value >= 100 { return symbol + String(format: "%.0f", value) }
        if value >= 1 { return symbol + String(format: "%.1f", value) }
        return symbol + String(format: "%.2f", value)
    }
}

// MARK: - 趋势明细表

struct TrendTable: View {
    let points: [TrendPoint]
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Card(title: "明细") {
            ForEach(points.reversed()) { p in
                HStack {
                    Text(p.label)
                        .font(.system(size: 12, design: .rounded))
                        .monospacedDigit()
                    Spacer()
                    Text(Format.tokens(p.total_tokens))
                        .font(.system(size: 11).monospacedDigit())
                        .foregroundStyle(Theme.textSecondary)
                    Text("\(p.requests) 次")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textTertiary)
                    Text(Format.cost(settings.currency == .cny ? p.cost_cny : p.cost_usd,
                                     settings.currency))
                        .font(.system(size: 11).monospacedDigit())
                        .foregroundStyle(Theme.mint)
                }
                Divider()
            }
        }
    }
}
