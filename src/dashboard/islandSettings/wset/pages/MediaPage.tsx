/**
 * 页⑤「媒体与歌词」：播放自动显示、滚动歌词、播放日志、歌词源回退顺序，
 * 以及同时有多个播放器时的识别优先级。
 */
import {
  Note,
  PageHeader,
  PageResetBar,
  Row,
  Section,
  StringList,
  Switch,
  TextInput,
} from "../controls";
import { PAGE_FIELDS, type PageKey } from "../schema";
import {
  readFlag,
  readStringList,
  readText,
  UnsupportedTag,
  type PageProps,
} from "./common";

const PAGE: PageKey = "media";

export function MediaPage({ draft }: PageProps) {
  return (
    <>
      <PageHeader
        title="媒体与歌词"
        desc="正在播放的识别方式、自动切换、歌词源与滚动歌词。"
      />

      <Section title="播放">
        <Row
          title="开始播放时自动显示正在播放"
          desc="暂停一段时间后会自动切回收起前的视图。"
          tag={<UnsupportedTag draft={draft} keyName="media.auto_show" />}
          rowKey="media.auto_show"
        >
          <Switch
            checked={readFlag(draft, "media.auto_show", true)}
            ariaLabel="开始播放时自动显示正在播放"
            onChange={(next) => draft.setValue("media.auto_show", next)}
          />
        </Row>
        <Row
          title="显示滚动歌词"
          desc="默认先查网易云，失败回退 LRCLIB / 自定义源；都没有时只显示标题。"
          tag={<UnsupportedTag draft={draft} keyName="media.lyrics" />}
          rowKey="media.lyrics"
        >
          <Switch
            checked={readFlag(draft, "media.lyrics", true)}
            ariaLabel="显示滚动歌词"
            onChange={(next) => draft.setValue("media.lyrics", next)}
          />
        </Row>
        <Row
          title="读取网易云本地播放日志"
          desc="在 Windows 上读取网易云音乐 3.x 的本地日志，用于显示当前歌曲、播放状态与进度。"
          tag={<UnsupportedTag draft={draft} keyName="media.netease_log" />}
          rowKey="media.netease_log"
        >
          <Switch
            checked={readFlag(draft, "media.netease_log", true)}
            ariaLabel="读取网易云本地播放日志"
            onChange={(next) => draft.setValue("media.netease_log", next)}
          />
        </Row>
        <Row
          wide
          title="其他播放器的播放日志"
          desc="添加播放器或桥接程序输出的 JSON / JSONL 文件完整路径。日志用于显示，播放控制需要系统媒体会话。"
          error={draft.errors["media.playback_logs"]}
          tag={<UnsupportedTag draft={draft} keyName="media.playback_logs" />}
          rowKey="media.playback_logs"
        >
          <StringList
            values={readStringList(draft, "media.playback_logs")}
            placeholder={String.raw`例如 C:\Music\now-playing.jsonl`}
            addLabel="添加路径"
            onChange={(next) => draft.setValue("media.playback_logs", next)}
          />
        </Row>
      </Section>

      <Section title="歌词源">
        <Row
          wide
          title="回退顺序"
          desc="用英文逗号分隔，按顺序尝试：netease（网易云）、lrclib、custom（自定义 API）。"
          error={draft.errors["media.lyrics_sources"]}
          tag={<UnsupportedTag draft={draft} keyName="media.lyrics_sources" />}
          rowKey="media.lyrics_sources"
        >
          <TextInput
            wide
            value={readText(draft, "media.lyrics_sources")}
            placeholder="netease,lrclib,custom"
            ariaLabel="回退顺序"
            onChange={(next) => draft.setValue("media.lyrics_sources", next)}
          />
        </Row>
        <Row
          wide
          title="自定义歌词 API"
          desc="可选。URL 支持 {title} {artist} {album} {duration} {song_id} 占位符；返回 LRC 文本，或 JSON 里的 lyrics / lrc / syncedLyrics 字段。"
          error={draft.errors["media.lyrics_custom_url"]}
          tag={<UnsupportedTag draft={draft} keyName="media.lyrics_custom_url" />}
          rowKey="media.lyrics_custom_url"
        >
          <TextInput
            wide
            value={readText(draft, "media.lyrics_custom_url")}
            placeholder="https://…?title={title}&artist={artist}"
            ariaLabel="自定义歌词 API"
            onChange={(next) => draft.setValue("media.lyrics_custom_url", next)}
          />
        </Row>
        <Note>
          LRCLIB 免费、无需 Key。默认网易云 → LRCLIB → 自定义 API；在回退顺序里删掉某个源即可关闭它。
        </Note>
      </Section>

      <Section title="播放器识别">
        <Row
          wide
          title="识别优先级"
          desc="同时有多个播放器时优先显示靠前的；可以填进程名或应用标识。"
          error={draft.errors["media.priority"]}
          tag={<UnsupportedTag draft={draft} keyName="media.priority" />}
          rowKey="media.priority"
        >
          <StringList
            values={readStringList(draft, "media.priority")}
            reorderable
            placeholder="例如 cloudmusic"
            addLabel="添加"
            onChange={(next) => draft.setValue("media.priority", next)}
          />
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
