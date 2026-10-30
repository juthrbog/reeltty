# reeltty InnerTube client: implementation spec

Status: verified against live responses on 2026-10-29. WEB client, no login, no PO token.

Fixtures: `tests/fixtures/innertube/` (a subset, with tracking fields removed). Every path in this spec was checked with jq against a fixture there. Use these fixtures as parser test data.

Notation:
- Paths are jq paths.
- `[]` means "every element". `[-1]` means "last element".
- `A // B` means "use A; if A is missing, use B".
- Short names are defined once per section (for example `L` = one `lockupViewModel` object).

---

## 1. Client context and headers

### 1.1 Transport

- Base URL: `https://www.youtube.com/youtubei/v1/<endpoint>?prettyPrint=false`
- Method: `POST`
- Endpoints used: `search`, `browse`, `next`, `player`, `navigation/resolve_url`.

### 1.2 Headers

| Header | Required | Notes |
|---|---|---|
| `Content-Type: application/json` | **Yes** | Without it the server returns HTTP 400 `FAILED_PRECONDITION` (`search_minimal_headers.json`). |
| `X-Goog-Visitor-Id` | No | Send the stored visitor ID (see 1.4). |
| `User-Agent` | No | Requests with no User-Agent returned full results (`search_no_ua_json_ct.json`, `next_normal_minimal_headers.json`). Send a normal browser UA anyway. |
| `X-Youtube-Client-Name: 1`, `X-Youtube-Client-Version` | No | Optional. Safe to send. |
| Cookies, `Origin` | No | Not sent in any test. |

### 1.3 Client context and API key

Request base body:

```json
{"context":{"client":{"clientName":"WEB","clientVersion":"2.20261029.01.00","hl":"en","gl":"US"}}}
```

- `clientVersion`: read `"INNERTUBE_CLIENT_VERSION":"..."` from the `https://www.youtube.com/` HTML. Value on 2026-10-29: `2.20261029.01.00`. Keep a hard-coded fallback.
- An old version (`2.20240101.00.00`) still works and returns the **same new view models** (`browse_playlist_oldclient.json`). The server chooses the renderer format. Pinning an old version does not bring back old renderers.
- API key: the home page also has an `INNERTUBE_API_KEY` value. **Not needed.** All requests without `key=` returned 200. Do not send it.
- `hl` controls UI text (tab titles, "views", "ago"). The parser rules below assume `hl=en`. Always send `hl=en` (see Parser risks).

### 1.4 visitorData

- First response with no visitor ID sent: `.responseContext.visitorData` holds a new ID. It is URL-encoded (ends in `%3D%3D`).
- Store it. Send it back **as-is** (with `%3D`) in the `X-Goog-Visitor-Id` header. Putting it in `context.client.visitorData` has the same effect.
- When you send a visitor ID, `.responseContext.visitorData` is **absent** in the response (`browse_channel_videos_with_visitor.json`). The ID is echoed at `.responseContext.serviceTrackingParams[] | select(.service=="GFEEDBACK") | .params[] | select(.key=="visitor_data") | .value`.
- Rust type: `visitor_data: Option<String>`. Update the stored value only when a response has one.

### 1.5 Error responses

HTTP error responses have a JSON body `{"error":{"code":<int>,"message":...,"status":"..."}}`.

| Case | Result | Fixture |
|---|---|---|
| Wrong content type | 400 `FAILED_PRECONDITION` | `search_minimal_headers.json` |
| Handle used as `browseId` | 400 `INVALID_ARGUMENT` | `browse_channel_handle_browseid_err.json` |
| Handle that does not exist (resolve_url) | 404 `NOT_FOUND` | `resolve_url_missing_handle.json` |

### 1.6 Continuation requests (all endpoints)

Same URL as the first page. Body is the base context plus only the token:

```json
{"context":{"client":{...}},"continuation":"<token>"}
```

- Do not send `query`, `params`, `browseId` or `videoId` with a continuation.
- Tokens may contain `%3D`. Send them unchanged. The server accepts them.
- There are **two continuation item types**. The parser must handle both:

| Type | Token path (relative to the item) | Where |
|---|---|---|
| `continuationItemRenderer` | `.continuationEndpoint.continuationCommand.token` | search, channel grids, related videos, comments |
| `continuationItemViewModel` | `.continuationCommand.innertubeCommand.continuationCommand.token` | playlists |

Rust helper: `fn continuation_token(item: &Value) -> Option<String>` that tries both paths.

---

## 2. Endpoints

### 2.1 `navigation/resolve_url` (handle or URL to browse ID)

Body: base + `"url":"https://www.youtube.com/@LinusTechTips"`.

| Field | Path |
|---|---|
| browse ID | `.endpoint.browseEndpoint.browseId` (`UC...` for a channel, `VL<playlistId>` for a playlist) |
| browse params | `.endpoint.browseEndpoint.params` (optional; a tab URL such as `/@X/videos` returns the Videos tab params) |
| page type | `.endpoint.commandMetadata.webCommandMetadata.webPageType` (`WEB_PAGE_TYPE_CHANNEL`, `WEB_PAGE_TYPE_PLAYLIST`) |
| vanity flag | `.endpoint.commandMetadata.resolveUrlCommandMetadata.isVanityUrl` |

- A handle cannot be sent as `browseId`. Always resolve it first.
- Fixtures: `resolve_url_handle.json`, `resolve_url_handle_videos.json`, `resolve_url_playlist.json`, `resolve_url_missing_handle.json`.

### 2.2 `search`

First page body:

```json
{"context":{...},"query":"rust programming","params":"EgIQAQ=="}
```

- Leave out `params` when there is no filter. See section 4 for the encoding.

Item lists (each element of `contents[]` is an item, see section 3):

| Response | Path |
|---|---|
| First page | `.contents.twoColumnSearchResultsRenderer.primaryContents.sectionListRenderer.contents[] \| .itemSectionRenderer.contents[]?` |
| Continuation page | `.onResponseReceivedCommands[0].appendContinuationItemsAction.continuationItems[] \| .itemSectionRenderer.contents[]?` |
| Chip reload | `.onResponseReceivedCommands[].reloadContinuationItemsCommand \| select(.slot=="RELOAD_CONTINUATION_SLOT_BODY") \| .continuationItems[0].twoColumnSearchResultsRenderer.primaryContents.sectionListRenderer.contents[] \| .itemSectionRenderer.contents[]?` |

Continuation token: the `continuationItemRenderer` that is a **sibling of** the `itemSectionRenderer`s (not inside them):

- First page: `.contents.twoColumnSearchResultsRenderer.primaryContents.sectionListRenderer.contents[] | .continuationItemRenderer.continuationEndpoint.continuationCommand.token // empty`
- Continuation page: `.onResponseReceivedCommands[0].appendContinuationItemsAction.continuationItems[] | .continuationItemRenderer.continuationEndpoint.continuationCommand.token // empty`
- Chip reload: the same `sectionListRenderer.contents[]` as the chip-reload item list.
- Filters stay active on continuation pages (`search_combo_week_video_over20_views_cont1.json`).

Other response fields:

| Field | Path | Notes |
|---|---|---|
| estimated results | `.estimatedResults` | A **string** (`"6192373"`). Parse to `u64`. |
| safe search | `.isSafeSearchTriggered` | Continuation pages only. |
| chips | `.header.searchHeaderRenderer.chipBar.chipCloudRenderer.chips[].chipCloudChipRenderer` | `.text.simpleText` and `.navigationEndpoint.continuationCommand.token`. Chips seen: All (no token), Shorts, Unwatched, Videos, Recently uploaded, Live. Send the token as a `continuation`. Chip tokens are opaque; do not build them. |
| filter dialog | `.header.searchHeaderRenderer.searchFilterButton.buttonRenderer.command.openPopupAction.popup.searchFilterOptionsDialogRenderer.groups[].searchFilterGroupRenderer.filters[].searchFilterRenderer` | See 4.4. |

A chip reload response has two `reloadContinuationItemsCommand`s: `RELOAD_CONTINUATION_SLOT_BODY` (items) and `RELOAD_CONTINUATION_SLOT_HEADER` (a new `searchHeaderRenderer`). Select by `slot`, not by index.

Spelling messages (items in the same list as videos):

| Renderer | Fields |
|---|---|
| `showingResultsForRenderer` (query was auto-corrected) | `.correctedQuery.runs`, `.originalQuery.simpleText`, `.originalQueryEndpoint.searchEndpoint.query` and `.params` (`QgIIAQ%3D%3D`, "search instead for") |
| `didYouMeanRenderer` (auto-correct turned off) | `.correctedQuery.runs`, `.correctedQueryEndpoint.searchEndpoint.query`. `.correctedQueryEndpoint.searchEndpoint.params` is present only when filters are active (`CAMSCRABkgEEOgJwAQ%3D%3D` in `search_misspelled_nospell_video_views.json`) and keeps them. Send it when present. |

No results: a nonsense query (`search_no_results.json`) still returned 3 loosely related `videoRenderer`s and a continuation token. No "no results" renderer was seen. The parser must still accept an empty item list.

### 2.3 `browse`: channel

Body: base + `"browseId":"UC..."` and optional `"params":"<tab params>"`.

**Tabs** (present on every tab response):
- `.contents.twoColumnBrowseResultsRenderer.tabs[] | (.tabRenderer // .expandableTabRenderer)`. The Search tab is an `expandableTabRenderer`.
- Fields: `.title`, `.selected`, `.endpoint.browseEndpoint.params`, `.endpoint.commandMetadata.webCommandMetadata.url`.
- `.selected` is `true` on the selected tab and **missing** on the others (the Search tab has `false`). Treat missing as `false`.
- **Match tabs by URL suffix**, not by title (titles follow `hl`).

| Tab | URL suffix | params seen (LTT) |
|---|---|---|
| Home | `/featured` | `EghmZWF0dXJlZPIGBAoCMgA%3D` (or omit params) |
| Videos | `/videos` | `EgZ2aWRlb3PyBgQKAjoA` |
| Shorts | `/shorts` | `EgZzaG9ydHPyBgUKA5oBAA%3D%3D` |
| Live | `/streams` | `EgdzdHJlYW1z8gYECgJ6AA%3D%3D` |
| Playlists | `/playlists` | `EglwbGF5bGlzdHPyBgoKCEIGCgIQaCIA` |

Also seen: Shows, Podcasts, Posts, Store, Search, Releases (`/releases`, Lofi Girl). Read params from the tab list when possible instead of hard-coding them. The server accepts params URL-encoded or decoded (`browse_channel_streams_pctparams.json`).

**Selected tab:** `T = .contents.twoColumnBrowseResultsRenderer.tabs[].tabRenderer | select(.selected)`.

**Grid tabs (Videos, Shorts, Live):**
- Items: `T.content.richGridRenderer.contents[] | .richItemRenderer.content` (one key: `lockupViewModel` or `shortsLockupViewModel`).
- Token: `T.content.richGridRenderer.contents[-1].continuationItemRenderer.continuationEndpoint.continuationCommand.token`. Missing when the first page is the last page (`browse_channel_streams_lofigirl.json`: 24 items, no continuation item).
- Continuation response: `.onResponseReceivedActions[].appendContinuationItemsAction.continuationItems[]`. Same `richItemRenderer` items, trailing `continuationItemRenderer`. `targetId` equals `T.content.richGridRenderer.targetId`. No header or metadata (`browse_channel_videos_cont1.json`).
- Chips: `T.content.richGridRenderer.header.chipBarViewModel.chips[].chipViewModel` with `.text`, `.selected`, `.tapCommand.innertubeCommand.continuationCommand.token`.
  - Videos tab (LTT): Latest / Members only / Public. The selected "Latest" chip on the Videos tab has no `.selected` and **no token**. Token is `Option`.
  - Shorts and Live tabs: Latest / Popular / Oldest.
  - Chip response: `.onResponseReceivedActions[].reloadContinuationItemsCommand`, slots `RELOAD_CONTINUATION_SLOT_HEADER` (new `chipBarViewModel`) and `RELOAD_CONTINUATION_SLOT_BODY` (items + continuation).
  - **The "Public" chip removes members-only videos on the server** (`browse_channel_videos_chip_public.json`: 0 members-only badges, against 10 of 30 without it).

**Home tab:** `T.content.sectionListRenderer.contents[].itemSectionRenderer.contents[]`, one shelf per section:
- `channelVideoPlayerRenderer` (trailer), `recognitionShelfRenderer`.
- `shelfRenderer`: title `.title.runs[0].text`, items `.content.horizontalListRenderer.items[]` (`lockupViewModel`, or `gridChannelRenderer` for featured channels, or `postRenderer` for posts).
- `reelShelfRenderer`: `.items[]` holds `shortsLockupViewModel`.

**Channel metadata** (every first-page tab response; not in grid continuation or chip responses):

| Field | Path |
|---|---|
| title | `.metadata.channelMetadataRenderer.title` |
| id | `.metadata.channelMetadataRenderer.externalId` |
| full description | `.metadata.channelMetadataRenderer.description` |
| avatar | `.metadata.channelMetadataRenderer.avatar.thumbnails[]` |
| vanity URL | `.metadata.channelMetadataRenderer.vanityChannelUrl` (`http://www.youtube.com/@X`) |
| handle, subscribers, video count | `[H.metadata.contentMetadataViewModel.metadataRows[].metadataParts[].text.content]` where `H = .header.pageHeaderRenderer.content.pageHeaderViewModel`. Gives `["@LinusTechTips","16.9M subscribers","7.9K videos"]`. Match by content: starts with `@`, contains `subscribers`, contains `videos`. |
| header avatar | `H.image.decoratedAvatarViewModel.avatar.avatarViewModel.image.sources[]` (72, 120, 160 px) |
| banner | `H.banner.imageBannerViewModel.image.sources[]` (6 sizes, 1060x175 to 2560x424) |
| verified | `H.title.dynamicTextViewModel.text.attachmentRuns[].element.type.imageType.image.sources[].clientResource.imageName`: `CHECK_CIRCLE_FILLED` (verified, LTT) or `AUDIO_BADGE` (artist, Lofi Girl). Missing when not verified (not captured). |

### 2.4 `browse`: playlist

Body: base + `"browseId":"VL<playlistId>"`.

- Items: `.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].itemSectionRenderer.contents[] | .lockupViewModel // empty`. 100 per page. No `playlistVideoRenderer` was seen.
- **Video token (use this):** `...itemSectionRenderer.contents[-1].continuationItemViewModel.continuationCommand.innertubeCommand.continuationCommand.token`.
  - Response: `.onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems[]` holds `lockupViewModel` **directly** (no `richItemRenderer` wrapper). `targetId` = playlist ID without `VL`.
  - Last page: no continuation item at the end.
- **Section token (ignore it):** `...sectionListRenderer.contents[1].continuationItemViewModel...token`. It returns related playlists (`horizontalShelfViewModel`), not videos. When nothing more exists, the response has no `onResponseReceivedActions` at all (`browse_playlist_cont1.json` has only `responseContext` and `trackingParams`).
- A short playlist with no more videos has only the section token.

Playlist metadata (`HP = .header.pageHeaderRenderer.content.pageHeaderViewModel`):

| Field | Path |
|---|---|
| title | `.metadata.playlistMetadataRenderer.title` |
| description | `.metadata.playlistMetadataRenderer.description`. **Optional**: missing on `browse_playlist_long.json`. |
| owner name, id, handle | `.sidebar.playlistSidebarRenderer.items[1].playlistSidebarSecondaryInfoRenderer.videoOwner.videoOwnerRenderer.title.runs[0]` → `.text`, `.navigationEndpoint.browseEndpoint.browseId`, `.navigationEndpoint.browseEndpoint.canonicalBaseUrl`. The last one is a handle only when it starts with `/@`; it can be `/channel/UC...` (`browse_playlist_long.json`, owner "Music"). |
| header rows (fallback) | `[HP.metadata.contentMetadataViewModel.metadataRows[].metadataParts[] \| .text.content // .avatarStack.avatarStackViewModel.text.content]` → `["by Google Search Stories","Playlist","13 videos","102,310 views"]` |
| owner (fallback) | `HP.metadata.contentMetadataViewModel.metadataRows[0].metadataParts[0].avatarStack.avatarStackViewModel.text.content` (`"by X"`, strip `by `) |
| stats | `.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.stats[]` read as `.simpleText // (.runs \| map(.text) \| join(""))`: "13 videos", "102,310 views", "Last updated on Jun 17, 2014" |
| thumbnail | `HP.heroImage.contentPreviewImageViewModel.image.sources[]` |
| hidden items alert | `.alerts[].alertWithButtonRenderer.text.simpleText` ("3 unavailable videos are hidden") |

### 2.5 `next` (watch page)

Body: base + `"videoId":"aircAruvnKk"`.

Let `R = .contents.twoColumnWatchNextResults`.

- `R.results.results.contents[]` holds, in order: `videoPrimaryInfoRenderer`, `videoSecondaryInfoRenderer`, `compositeVideoPrimaryInfoRenderer` (empty), optional `merchandiseShelfRenderer`, `itemSectionRenderer`. **Select by key, never by index** (the merch shelf is missing in `next_normal.json`).
- `P = R.results.results.contents[] | .videoPrimaryInfoRenderer // empty`
- `S = R.results.results.contents[] | .videoSecondaryInfoRenderer // empty`

| Field | Path | Notes |
|---|---|---|
| title | `P.title.runs \| map(.text) \| join("")` | |
| views (VOD) | `P.viewCount.videoViewCountRenderer.viewCount.simpleText` | "25,037,755 views". Short: `.shortViewCount.simpleText`. |
| views (live) | `P.viewCount.videoViewCountRenderer.viewCount.runs` | `["10,132"," watching now"]` |
| is live | `P.viewCount.videoViewCountRenderer.isLive` | `true` or missing. `P.badges` is null even when live. |
| date text | `P.dateText.simpleText` | "Oct 5, 2017", "Started streaming on Aug 19, 2026", "Streamed live on Jul 12, 2022" |
| relative date | `P.relativeDateText.simpleText` | missing when live |
| like count | `P.videoActions.menuRenderer.topLevelButtons[] \| .segmentedLikeDislikeButtonViewModel // empty \| .likeButtonViewModel.likeButtonViewModel.toggleButtonViewModel.toggleButtonViewModel.defaultButtonViewModel.buttonViewModel` | `.title` = "568K"; exact number in `.accessibilityText` ("like this video along with 568,375 other people") |
| super title link | `P.superTitleLink.runs[0]` | Optional (missing on live). `.text` plus a `navigationEndpoint` that is **either** `browseEndpoint.browseId` (`VLPL...`, a series playlist, `next_chapters.json`) **or** `searchEndpoint` (a location or hashtag: "SAN DIEGO ZOO" in `next_normal.json`). Treat it as a series only when `browseEndpoint` is present. |
| channel name | `S.owner.videoOwnerRenderer.title.runs[0].text` | |
| channel id | `S.owner.videoOwnerRenderer.navigationEndpoint.browseEndpoint.browseId` | |
| channel handle | `S.owner.videoOwnerRenderer.navigationEndpoint.browseEndpoint.canonicalBaseUrl` | "/@3blue1brown" |
| subscribers | `S.owner.videoOwnerRenderer.subscriberCountText.simpleText` | "8.73M subscribers" |
| verified | `S.owner.videoOwnerRenderer.badges[].metadataBadgeRenderer.style` | `BADGE_STYLE_TYPE_VERIFIED` or `..._VERIFIED_ARTIST` |
| description | `S.attributedDescription` | See 2.5.1 |
| duration | not in `/next` | Use `/player` (2.6). |

#### 2.5.1 Description (`S.attributedDescription`)

- `.content`: full plain text. YouTube shortens visible link text ("https://www.3blue1brown.com/topics/ne...").
- `.commandRuns[]`: `{startIndex, length, onTap.innertubeCommand}`. The command has one of:
  - `urlEndpoint.url`: a `https://www.youtube.com/redirect?...&q=<url-encoded real URL>`. Read the real URL from `q`.
  - `watchEndpoint.{videoId, startTimeSeconds}`: a timestamp or a link to a video. `startTimeSeconds` can be missing for 0:00.
  - `browseEndpoint`: channels and hashtags (not seen, but expected).
- `.attachmentRuns[]`, `.decorationRuns[]`, `.styleRuns[]`: icons and styling. Optional. Ignore them in v1.
- **`startIndex` and `length` are UTF-16 code units.** Verified in `next_live.json`: UTF-16 slicing gives `https://lnk.to/halloween-lofi02`; code-point slicing gives `ttps://lnk.to/halloween-lofi02\n`.

#### 2.5.2 Related videos

- Items: `R.secondaryResults.secondaryResults.results[]`. Only `lockupViewModel` (contentType VIDEO or COURSE) was seen, plus a trailing `continuationItemRenderer`.
- Token: `R.secondaryResults.secondaryResults.results[-1].continuationItemRenderer.continuationEndpoint.continuationCommand.token`.
- Continuation response: `.onResponseReceivedEndpoints[] | .appendContinuationItemsAction // empty | select(.targetId=="watch-next-feed") | .continuationItems[]` (20 lockups + next token).
- Related lockups use V2 paths. Row 0 (channel name) has **no `commandRuns`**, so the channel id comes only from `M.image.decoratedAvatarViewModel...browseId` (95 of 99). Collabs use `M.image.avatarStackViewModel` (4 of 99) and row 0 reads "A and B" or "A and 2 more".
- Fallback source in the old format: `.playerOverlays.playerOverlayRenderer.endScreen.watchNextEndScreenRenderer.results[]`: `endScreenVideoRenderer` (`videoId`, `title.simpleText`, `lengthText.simpleText`, `lengthInSeconds` as a number) and `endScreenPlaylistRenderer` (`playlistId`, `title`, `videoCountText`, `longBylineText`, `thumbnail`; 1 in `next_chapters.json`).

#### 2.5.3 Comments (v1.1, paths only)

- First token: `R.results.results.contents[] | .itemSectionRenderer // empty | select(.sectionIdentifier=="comment-item-section") | .contents[0].continuationItemRenderer.continuationEndpoint.continuationCommand.token`.
- Count and sort tokens: in the engagement panel `engagement-panel-comments-section`, under `.header.engagementPanelTitleHeaderRenderer` (`.contextualInfo.runs[0].text`; `.menu.sortFilterSubMenuRenderer.subMenuItems[]`).
- Live videos have no comments section. Live chat token: `R.conversationBar.liveChatRenderer.continuations[0].reloadContinuationData.continuation`.

### 2.6 `player` (metadata only)

Body: base + `"videoId":"..."`.

- Without a PO token, `.playabilityStatus.status` is `"UNPLAYABLE"` ("Video unavailable") and there is **no `streamingData`**. Streams stay with yt-dlp.
- Metadata is still returned, **for both VOD and live** (`player_chapters.json`, `player_live.json`, `player_live_visitor.json`):

| Field | Path | Notes |
|---|---|---|
| duration | `.videoDetails.lengthSeconds` | String. `"0"` for a live stream. |
| ISO publish date | `.microformat.playerMicroformatRenderer.publishDate` | `"2017-10-05T08:11:25-07:00"`. Also `uploadDate`. |
| channel | `.videoDetails.channelId`, `.videoDetails.author` | |
| views | `.videoDetails.viewCount` | String integer. |
| live | `.videoDetails.isLive`, `.videoDetails.isLiveContent`, `.microformat.playerMicroformatRenderer.liveBroadcastDetails.{isLiveNow,startTimestamp}` | `isLive` exists only on live videos. |
| thumbnails | `.videoDetails.thumbnail.thumbnails[]` | 5 sizes, 168x94 up to 1920x1080. Best source for the detail view. |
| other | `.videoDetails.{title,keywords,shortDescription}`, `.microformat.playerMicroformatRenderer.{category,isFamilySafe,likeCount,availableCountries}` | `keywords` is optional (missing in `player_chapters.json`). |

Note: the `next` report said live `/player` has no `videoDetails`. The fixtures show it does. This spec follows the fixtures.

---

## 3. Item model

### 3.1 Rust shape

```rust
pub enum Item {
    Video(Video),
    Channel(Channel),
    Playlist(Playlist),
}

pub struct ChannelRef { pub id: Option<String>, pub name: String, pub handle: Option<String> }

pub struct Thumbnail { pub url: String, pub width: u32, pub height: u32 }

pub enum VideoKind { Normal, Short, Live, PastStream }

pub struct Video {
    pub id: String,
    pub title: String,
    pub kind: VideoKind,
    pub channels: Vec<ChannelRef>,     // empty when unknown; 2+ for collabs
    pub duration_text: Option<String>, // "13:59:10"
    pub duration_secs: Option<u32>,    // parsed from duration_text
    pub views_text: Option<String>,    // raw text, "1.2M views" / "32 watching"
    pub published_text: Option<String>,// "3 years ago" / "Streamed 1 day ago"
    pub thumbnails: Vec<Thumbnail>,
    pub members_only: bool,
    pub badges: Vec<String>,           // "4K", "CC", "New", ...
    pub description_snippet: Option<String>,
    pub playlist_index: Option<u32>,   // playlists only, 0-based
}

pub struct Channel {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    pub subscribers_text: Option<String>,
    pub description_snippet: Option<String>,
    pub verified: bool,
    pub thumbnails: Vec<Thumbnail>,
}

pub enum PlaylistKind { Playlist, Course }

pub struct Playlist {
    pub id: String,                    // PL... (no VL prefix)
    pub title: String,
    pub kind: PlaylistKind,
    pub channel: Option<ChannelRef>,
    pub count_text: Option<String>,    // "44 videos" / "9 lessons"
    pub thumbnails: Vec<Thumbnail>,
    pub first_video_id: Option<String>,
}
```

Unknown renderer keys must be **skipped, not errors**. Log the key name at debug level.

### 3.2 Item dispatch

The item object has one meaningful key. Dispatch on it:

| Key | Item kind | Seen in |
|---|---|---|
| `videoRenderer` | Video | search |
| `lockupViewModel` with `contentType` `LOCKUP_CONTENT_TYPE_VIDEO` | Video | browse (grids, home, playlist), next (related). Not in search. |
| `lockupViewModel` with `LOCKUP_CONTENT_TYPE_SHORT` | Video (Short) | channel home only |
| `lockupViewModel` with `LOCKUP_CONTENT_TYPE_PLAYLIST` / `_COURSE` | Playlist | search, browse home, next (COURSE) |
| `shortsLockupViewModel` | Video (Short) | search shelves, Shorts tab, home `reelShelfRenderer` |
| `channelRenderer` | Channel | search |
| `gridChannelRenderer` | Channel | channel home (featured channels) |
| `richItemRenderer` | unwrap `.content`, then dispatch | channel grids |
| `gridShelfViewModel` | container: dispatch each `.contents[]` | search (Shorts shelf). Title: `.header.sectionHeaderViewModel.headline.content` |
| `shelfRenderer` | container: `.content.verticalListRenderer.items[]` (search) or `.content.horizontalListRenderer.items[]` (home) | search, home. Title: `.title.runs[0].text` or `.title.simpleText` |
| `reelShelfRenderer` | container: `.items[]` | home |
| `continuationItemRenderer`, `continuationItemViewModel` | token, see 1.6 | everywhere |
| `showingResultsForRenderer`, `didYouMeanRenderer` | spelling message | search |
| `horizontalShelfViewModel` | related playlists; skip in v1 | playlist section continuation (`browse_playlist_long_sectioncont.json`) |
| `postRenderer`, `channelVideoPlayerRenderer`, `recognitionShelfRenderer` | skip | channel home |
| `endScreenVideoRenderer`, `endScreenPlaylistRenderer` | fallback only, see 2.5.2 | next (`playerOverlays`) |

Not seen in any fixture (support is optional): `compactVideoRenderer`, `gridVideoRenderer`, `playlistVideoRenderer`, `playlistRenderer`, `radioRenderer`, `reelItemRenderer`, `movieRenderer`, ad renderers.

### 3.3 Video fields per variant

#### Variant V1: `videoRenderer` (search). Let `V = .videoRenderer`.

| Field | Path |
|---|---|
| id | `V.videoId` |
| title | `V.title.runs \| map(.text) \| join("")` |
| channel (single) | `V.longBylineText.runs[0]` → name `.text`, id `.navigationEndpoint.browseEndpoint.browseId`, handle `.navigationEndpoint.browseEndpoint.canonicalBaseUrl` |
| channels (collab) | When `.navigationEndpoint.browseEndpoint` is missing: `V.longBylineText.runs[0].navigationEndpoint.showDialogCommand.panelLoadingStrategy.inlineContent.dialogViewModel.customContent.listViewModel.listItems[].listItemViewModel` → name `.title.content`, id `.rendererContext.commandContext.onTap.innertubeCommand.browseEndpoint.browseId`, handle from `.subtitle.content` ("@handle • N subscribers", wrapped in Unicode bidi marks, see risks). 14 collab videos seen. |
| duration | `V.lengthText.simpleText`. Fallback: `V.thumbnailOverlays[].thumbnailOverlayTimeStatusRenderer.text.simpleText`. Missing on live. |
| views | `V.viewCountText.simpleText // (V.viewCountText.runs \| map(.text) \| join(""))`. Live: runs `["32"," watching"]`. |
| published | `V.publishedTimeText.simpleText`. Missing on live. |
| thumbnails | `V.thumbnail.thumbnails[]` `{url,width,height}`. Usually 360x202 + 720x404 (564 of 571), else one 480x270. |
| live | `V.badges[].metadataBadgeRenderer.style == "BADGE_STYLE_TYPE_LIVE_NOW"` |
| past stream | published text starts with `Streamed` |
| badges | `V.badges[].metadataBadgeRenderer.label`. Labels seen: 4K, New, CC, LIVE, Beginner, Overview, "Fundraiser" (style `BADGE_STYLE_TYPE_YPC`) and shopping badges "N products" (style `BADGE_STYLE_TYPE_SIMPLE`). Drop labels that match `^\d+ products$`; drop LIVE (it sets `kind`). |
| members only | `V.badges[].metadataBadgeRenderer.style == "BADGE_STYLE_TYPE_MEMBERS_ONLY"` or label "Members only" (**not seen**; rule from yt-dlp) |
| verified channel | `V.ownerBadges[].metadataBadgeRenderer.style` starts with `BADGE_STYLE_TYPE_VERIFIED` |
| description snippet | `V.detailedMetadataSnippets[0].snippetText.runs \| map(.text) \| join("")`. Fallback: `V.descriptionSnippet.runs`. Missing on 5 of 571; 4 of those have neither. |
| channel avatar | `V.channelThumbnailSupportedRenderers.channelThumbnailWithLinkRenderer.thumbnail.thumbnails[0].url`. Also on all 571: `V.avatar.decoratedAvatarViewModel.avatar.avatarViewModel.image.sources[]` (68x68). |

#### Variant V2: `lockupViewModel` VIDEO (browse grids, home, playlists, related)

Let `L = .lockupViewModel`, `M = L.metadata.lockupMetadataViewModel`, `ROWS = M.metadata.contentMetadataViewModel.metadataRows`, `BADGE = L.contentImage.thumbnailViewModel.overlays[].thumbnailBottomOverlayViewModel.badges[].thumbnailBadgeViewModel`.

| Field | Path |
|---|---|
| id | `L.contentId` (= `L.rendererContext.commandContext.onTap.innertubeCommand.watchEndpoint.videoId`) |
| title | `M.title.content` |
| thumbnails | `L.contentImage.thumbnailViewModel.image.sources[]`. Sizes by context: grid 360x202 + 720x404; playlist 168x94, 196x110, 246x138, 336x188; related 168x94 + 336x188. |
| duration | `BADGE.text` where `BADGE.badgeStyle == "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT"` ("11:23") |
| live | `BADGE.badgeStyle == "THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE"` (text "LIVE") |
| views, published | parts in `ROWS[].metadataParts[].text.content`, **classified by content** (see 3.6) |
| members only | `ROWS[].badges[].badgeViewModel.badgeStyle == "BADGE_MEMBERS_ONLY"` (text "Members only") |
| badges | `ROWS[].badges[].badgeViewModel.badgeText` ("CC", "New") |
| channel (single) | id `M.image.decoratedAvatarViewModel.rendererContext.commandContext.onTap.innertubeCommand.browseEndpoint.browseId`; name from the channel-name row (see below) |
| channels (collab) | `M.image.avatarStackViewModel.rendererContext.commandContext.onTap.innertubeCommand.showDialogCommand.panelLoadingStrategy.inlineContent.dialogViewModel.customContent.listViewModel.listItems[].listItemViewModel` → `.title.content`, `.rendererContext.commandContext.onTap.innertubeCommand.browseEndpoint.browseId`. Seen on related and channel home (15 lockups, 2-3 channels each). **Not on channel grids**: there `M.image` is null and the "A and B" row has no `commandRuns`, so only the joined names exist (4 in `browse_channel_*`). |
| playlist index | `L.rendererContext.commandContext.onTap.innertubeCommand.watchEndpoint.index` (0-based; playlists) |

Row layout differs by context:

| Context | Rows |
|---|---|
| Channel Videos/Live tab | `["1.2M views","20 hours ago"]`. **No channel row, `M.image` is null.** Use the browsed channel. Members-only: `["1 day ago"]` + a badge row. Collab: extra first row with "A and B". |
| Playlist | row 0 = channel name (`ROWS[0].metadataParts[0].text.content`, id at `.text.commandRuns[0].onTap.innertubeCommand.browseEndpoint.browseId`); row 1 = `["772M views","2 years ago"]` |
| Channel home shelves | Mixed. Some lockups have a row 0 with the channel name (the browsed channel, "Linus Tech Tips", or another one, "Techquickie"), some do not (`["1.2M views","2 years ago"]` first). Members-only and `BADGE_DEFAULT` badge rows appear. |
| Related (`/next`) | row 0 = channel name (no `commandRuns`); row 1 = `["909K","1mo ago"]` (**short form, no "views" word**). Use `.accessibilityLabel` on each part for long text ("909 thousand views", "1 month ago"). Live: `["1.8K watching"]`. Optional row 2 = badges. |

#### Variant V3: `shortsLockupViewModel`. Let `S = .shortsLockupViewModel`.

| Field | Path |
|---|---|
| id | `S.onTap.innertubeCommand.reelWatchEndpoint.videoId` (also in `S.entityId` = `shorts-shelf-item-<id>`) |
| title | `S.overlayMetadata.primaryText.content` |
| views | `S.overlayMetadata.secondaryText.content` ("84K views") |
| thumbnails | `S.thumbnailViewModel.thumbnailViewModel.image.sources[]` (nested twice; portrait). Sizes vary: usually 405x720 + 405x608, also 404x720, 334x480, 270x480, sometimes only one source. |
| badge | `S.badge.badgeViewModel.badgeText` ("New"). Optional (19 of 413 in search). |
| kind | always `Short` |
| channel, duration, published | **not present.** On a channel Shorts tab, use the browsed channel. |

#### Variant V4: `lockupViewModel` SHORT (channel home only)

Same paths as V2. Set kind `Short`. Its tap URL is `/watch?v=`, not `/shorts/`.

### 3.4 Channel fields per variant

#### Variant C1: `channelRenderer` (search). Let `C = .channelRenderer`.

| Field | Path |
|---|---|
| id | `C.channelId` |
| name | `C.title.simpleText` |
| handle | `C.navigationEndpoint.browseEndpoint.canonicalBaseUrl` (`/@jonhoo`). **Also** in `C.subscriberCountText.simpleText` (`@jonhoo`). |
| subscribers | **`C.videoCountText.simpleText`** ("111K subscribers"). The field names are swapped; verified on all 23 channels. |
| description | `C.descriptionSnippet.runs \| map(.text) \| join("")` |
| verified | `C.ownerBadges[].metadataBadgeRenderer.style` starts with `BADGE_STYLE_TYPE_VERIFIED` |
| thumbnails | `C.thumbnail.thumbnails[]`. URLs start with `//`; prefix `https:`. |

Defensive rule: decide which text is the handle (starts with `@`) and which is the count (contains `subscriber`) by content, not by field name.

#### Variant C2: `gridChannelRenderer` (channel home, featured channels). Let `G = .gridChannelRenderer`.

Verified on the 12 items in `browse_channel_home.json`:

| Field | Path |
|---|---|
| id | `G.channelId` |
| name | `G.title.simpleText` |
| handle | `G.navigationEndpoint.browseEndpoint.canonicalBaseUrl` (`/@techquickie`) |
| subscribers | `G.subscriberCountText.simpleText` ("80.5K subscribers"). Not swapped here, unlike C1. |
| video count | `G.videoCountText.runs \| map(.text) \| join("")` ("1,305 videos"). Optional (missing on 3). |
| verified | `G.ownerBadges[].metadataBadgeRenderer.style` starts with `BADGE_STYLE_TYPE_VERIFIED`. Optional. |
| thumbnails | `G.thumbnail.thumbnails[]`. URLs start with `//`; prefix `https:`. |

#### Variant C3: channel page header (`browse`)

See 2.3 "Channel metadata".

### 3.5 Playlist fields per variant

#### Variant P1: `lockupViewModel` PLAYLIST / COURSE (search, home, related)

Let `L`, `M`, `ROWS` as in V2, and `CT = L.contentImage.collectionThumbnailViewModel.primaryThumbnail.thumbnailViewModel`.

| Field | Path |
|---|---|
| id | `L.contentId`. Equal to `L.rendererContext.commandContext.onTap.innertubeCommand.watchEndpoint.playlistId` on all 79 playlist/course lockups in search, home and related. |
| kind | `L.contentType`: `LOCKUP_CONTENT_TYPE_PLAYLIST` → Playlist, `LOCKUP_CONTENT_TYPE_COURSE` → Course. Use this, not the kind label. |
| title | `M.title.content` |
| channel name | Search: `ROWS[0].metadataParts[0].text.content`. **Channel home: not present** (row 0 is just `["Playlist"]`); use the browsed channel. |
| channel id | `ROWS[0].metadataParts[0].text.commandRuns[0].onTap.innertubeCommand.browseEndpoint.browseId`. On channel home this run is on the "Playlist" text itself. |
| kind label | Search: `ROWS[0].metadataParts[1].text.content` ("Playlist" or "Course"). Can disagree with `contentType` (2 COURSE lockups in search say "Playlist"). |
| browse id | The part with text "View full playlist" / "View full course" has `.text.commandRuns[0].onTap.innertubeCommand.browseEndpoint.browseId` = `VL<id>`. |
| count | `CT.overlays[] \| .thumbnailOverlayBadgeViewModel // empty \| .thumbnailBadges[].thumbnailBadgeViewModel.text` ("44 videos", "1 video", "9 lessons"). The `// empty` is needed: the other overlay (`thumbnailHoverOverlayViewModel`) has no badges and a plain `[]` path errors in jq. |
| thumbnails | `CT.image.sources[]` (search: 360x202 + 720x404; related course: 320x180 + 800x450 on `i9.ytimg.com`) |
| first video | `L.rendererContext.commandContext.onTap.innertubeCommand.watchEndpoint.videoId` |

Row layout in search (66 lockups): row 0 = `[channel, kind label]`; row 1 = `{isSpacerRow}` (no `metadataParts`); rows 2-3 = first two videos ("title · 7:10"); row 4 = "View full playlist" / "View full course"; optional row 5 = course description. One playlist had an extra row "Updated 3 days ago" before the spacer. Row layout on channel home: row 0 = `["Playlist"]`, row 1 = "View full playlist". Find parts by content, never by row index.

#### Variant P2: playlist page header (`browse VL...`)

See 2.4 "Playlist metadata".

### 3.6 Text classification rules (lockup rows)

Apply to each `metadataParts[].text.content` (use `accessibilityLabel` when present for related videos):

| Rule | Meaning |
|---|---|
| ends with ` views` or ` view`, or `accessibilityLabel` ends with `views` | view count |
| contains ` watching` | live viewer count, set `kind = Live` |
| ends with ` ago`, or starts with `Streamed` | published text. `Streamed` → `PastStream` (unless Live). |
| otherwise, in row 0 of a playlist or related lockup | channel name |
| otherwise, in a channel-grid row before the views row | collab channel names ("A and B"), no ids |
| equals `Playlist`, `Course`, `View full playlist`, `View full course`, or starts with `Updated ` | playlist lockup label, not a date or channel |

Parse durations `H:MM:SS` / `M:SS` to seconds. Keep the raw text too.

---

## 4. Search filter `params` encoding

### 4.1 Format

`params` = base64 of a protobuf message. Standard alphabet with padding works when sent. Values copied from responses are URL-encoded (`%3D`); URL-decode before you decode them.

```
message SearchParams {
  1: sort (varint)            0=relevance  3=view count  (1=rating, 2=upload date: ignored by server)
  2: Filters (message) {
     1: upload_date           1=last hour  2=today  3=this week  4=this month  5=this year
     2: type                  1=video  2=channel  3=playlist  4=movie  9=shorts
     3: duration              1=<4 min (old)  2=>20 min  3=4-20 min (old)  4=<3 min  5=3-20 min
     4: HD  5: subtitles/CC  6: Creative Commons  7: 3D  8: live  9: purchased
     14: 4K  15: 360°  23: location  25: HDR  26: VR180        (each a bool, value 1)
  }
  8: { 1: 1 }                 turn off spelling auto-correct
}
```

Encoding rules:
- All filters go in one field-2 sub-message. Write fields in ascending field order.
- Key byte = `(field << 3) | wire_type`, written as a varint. Fields 16 and up need two bytes (field 23 varint = `b8 01`, 25 = `c8 01`, 26 = `d0 01`). Use a general varint encoder.
- Omit `params` entirely when no filter and default sort.
- No protobuf crate is needed. A 30-line encoder for varint and length-delimited fields is enough.

### 4.2 Verified single values

| Filter | params | Verified result |
|---|---|---|
| type: video | `EgIQAQ==` | videos only |
| type: channel | `EgIQAg==` | channels only |
| type: playlist | `EgIQAw==` | playlists and courses only |
| type: shorts | `EgIQCQ==` | Shorts shelves only |
| upload: last hour | `EgIIAQ==` | 3 results, 31-38 min old. Not in UI. |
| upload: today | `EgIIAg==` | 28 min to 23 h |
| upload: this week | `EgIIAw==` | 1-6 days (one outlier) |
| upload: this month | `EgIIBA==` | up to 4 weeks |
| upload: this year | `EgIIBQ==` | up to 10 months |
| duration: under 3 min | `EgIYBA==` | 93-175 s; UI selected |
| duration: 3-20 min | `EgIYBQ==` | 227-990 s; UI selected |
| duration: over 20 min | `EgIYAg==` | 1242-50350 s; UI selected |
| duration: under 4 min (old) | `EgIYAQ==` | 93-231 s; not shown selected |
| duration: 4-20 min (old) | `EgIYAw==` | 243-1155 s; not shown selected |
| feature: live | `EgJAAQ==` | live results, `BADGE_STYLE_TYPE_LIVE_NOW` |
| sort: view count | `CAM=` | views roughly descending; UI "Popularity" selected |
| sort: relevance | omit (or `CAA=`) | default |
| sort: upload date | `CAI=` | **ignored by server** |
| sort: rating | `CAE=` | **not honoured** |
| no auto-correct | `QgIIAQ==` | `didYouMeanRenderer` instead of `showingResultsForRenderer` |

### 4.3 Verified combinations

| params | Meaning | Result |
|---|---|---|
| `CAMSBggDEAEYAg==` | view count + week + video + over 20 min | all four shown selected; all results 1497-26641 s, 3-7 days old |
| `EgQQARgD` | video + 4-20 min | 243-990 s |
| `EgQIBRAD` | year + playlist | both shown selected |
| `EgYQASgBcAE=` | video + CC + 4K | all 20 results have "4K" and "CC" badges |
| `CAMSAhABQgIIAQ==` | view count + video + no auto-correct | Videos and Popularity shown selected |

### 4.4 Server-built params (filter dialog)

Every option in the search filter dialog (path in 2.2) has `.label.simpleText`, an optional `.status` and an optional `.navigationEndpoint.searchEndpoint.params`. `.status` is `FILTER_STATUS_SELECTED`, `FILTER_STATUS_DISABLED`, or missing (not selected). The params equal the current params with that option toggled. Groups (`.groups[].searchFilterGroupRenderer.title.simpleText`): Type (5), Duration (3), Upload date (4), Features (11), Prioritize (2: Relevance, Popularity). From `search_combo_week_video_over20_views.json` (current = view count + week + video + over 20):

| Option | status | params |
|---|---|---|
| Videos | `FILTER_STATUS_SELECTED` | `CAMSBAgDGAI%3D` (removes video) |
| This week | `FILTER_STATUS_SELECTED` | `CAMSBBABGAI%3D` |
| Over 20 minutes | `FILTER_STATUS_SELECTED` | `CAMSBAgDEAE%3D` |
| Popularity | `FILTER_STATUS_SELECTED` | none (the selected sort cannot be toggled off; pick Relevance instead) |
| Today | | `CAMSBggCEAEYAg%3D%3D` |
| 4K | | `CAMSCAgDEAEYAnAB` |
| HD | | `CAMSCAgDEAEYAiAB` |
| Subtitles/CC | | `CAMSCAgDEAEYAigB` |
| Creative Commons | | `CAMSCAgDEAEYAjAB` |
| 3D | | `CAMSCAgDEAEYAjgB` |
| Live | | `CAMSCAgDEAEYAkAB` |
| Purchased | | `CAMSCAgDEAEYAkgB` |
| 360° | | `CAMSCAgDEAEYAngB` |
| Location | | `CAMSCQgDEAEYArgBAQ%3D%3D` |
| HDR | | `CAMSCQgDEAEYAsgBAQ%3D%3D` |
| VR180 | | `CAMSCQgDEAEYAtABAQ%3D%3D` |
| Movies | | `CAMSBggDEAQYAg%3D%3D` |
| Relevance | | `CAASBggDEAEYAg%3D%3D` |
| Under 3 minutes | | `CAMSBggDEAEYBA%3D%3D` |
| 3 - 20 minutes | | `CAMSBggDEAEYBQ%3D%3D` |
| This month | | `CAMSBggEEAEYAg%3D%3D` |
| This year | | `CAMSBggFEAEYAg%3D%3D` |
| Shorts, Channels, Playlists | `FILTER_STATUS_DISABLED` | none (a duration filter is active) |

Use these values as encoder unit tests. They confirm the field numbers above.

### 4.5 Upload-date sort replacement

Sort by upload date is ignored. Use the "Recently uploaded" chip (2.2) instead. It returns recent videos in relevance order, not strict date order (`search_chip_recently_uploaded.json`: 19 videos).

---

## 5. Chapters

Source 1 (preferred): player bar, in `/next`.

```
.playerOverlays.playerOverlayRenderer.decoratedPlayerBarRenderer.decoratedPlayerBarRenderer.playerBar.multiMarkersPlayerBarRenderer.markersMap[]
  | select(.key=="DESCRIPTION_CHAPTERS") | .value.chapters[].chapterRenderer
```

| Field | Path | Example |
|---|---|---|
| title | `.title.simpleText` | "Introduction example" |
| start | `.timeRangeStartMillis` (integer ms) | 0, 67000, ..., 1023000 |
| thumbnails | `.thumbnail.thumbnails[]` | 2 sizes |

- Select by `key`. Other keys exist (`ANIMATION_ANNOTATION_MARKERS`, quiz markers).
- No chapters: `markersMap` is missing (live video) or has no `DESCRIPTION_CHAPTERS` entry. Return an empty list.
- End of a chapter = start of the next one; last chapter ends at the video duration from `/player`.

Source 2 (fallback): chapters engagement panel.

```
.engagementPanels[].engagementPanelSectionListRenderer
  | select(.panelIdentifier=="engagement-panel-macro-markers-description-chapters")
  | .content.macroMarkersListRenderer.contents[].macroMarkersListItemRenderer
```

Fields: `.title.simpleText`, `.timeDescription.simpleText` ("1:07"), `.onTap.watchEndpoint.startTimeSeconds` (67), `.thumbnail.thumbnails[]`.

Both sources gave 12 identical chapters for aircAruvnKk (`next_chapters.json`) and 3 for jNQXAC9IVRw (`next_normal.json`). Do not use `frameworkUpdates...macroMarkersListEntity`: it holds heatmap data, not chapters.

---

## 6. Parser risks

1. **Renderers change without notice.** The server picks the format; client version does not control it. Old renderers (`playlistVideoRenderer`, `compactVideoRenderer`, `gridVideoRenderer`) are gone today but may return for some users or regions. Skip unknown keys; never fail a whole page because of one item.
2. **`channelRenderer` field names are swapped.** `subscriberCountText` holds the handle and `videoCountText` holds the subscriber count. Classify by content (`@` prefix vs "subscriber").
3. **Lockup rows have no fixed layout.** Channel row present or not, members-only rows without views, collab rows, badge rows. Classify each part by content (3.6), never by index.
4. **Text rules depend on English.** "views", "watching", "ago", "Streamed", "subscribers" need `hl=en`. Always send `hl=en`.
5. **Related-video counts are short form** ("909K", "1mo ago") with no "views" word. Use `accessibilityLabel`.
6. **UTF-16 offsets** in `attributedDescription.commandRuns`. Slicing a Rust `&str` by these numbers gives wrong text or a panic at a char boundary. Convert via `encode_utf16()`.
7. **Two continuation types** (`continuationItemRenderer`, `continuationItemViewModel`) with different token paths. Playlists also have a second, section-level token that returns related playlists, not videos.
8. **Continuation containers differ by endpoint:** search uses `onResponseReceivedCommands`, browse uses `onResponseReceivedActions`, next uses `onResponseReceivedEndpoints`. Browse grid items are wrapped in `richItemRenderer`; playlist continuation items are not.
9. **End of list is shown in different ways:** no trailing continuation item, or no `onResponseReceivedActions` key at all. Treat both as the end.
10. **Collab videos have no single channel.** The byline is "A and B" with no `browseEndpoint`. Channels live in a dialog list. `Video.channels` must be a `Vec`.
11. **Collab subtitles contain Unicode bidi marks** (U+200E, U+2068, U+2069) around the handle and count, for example `‎⁨@GrinenkoPro⁩ • ⁨1.29K subscribers⁩`. Strip them before matching `@`.
12. **Shorts have no channel, duration or date.** UI must handle missing fields.
13. **Protocol-relative thumbnails** (`//yt3...`) on `channelRenderer` and `gridChannelRenderer`. Prefix `https:`. Video thumbnail URLs carry a query string (`?sqp=...&rs=...`) and use several hosts and paths (`i.ytimg.com/vi/`, `i.ytimg.com/eb/vi/`, `i9.ytimg.com`, file names `hq720`, `hqdefault`, `oar2`, `sardefault`, `*_custom_N`). Use the URL exactly as given; do not build it from the video id.
14. **Number fields are strings:** `estimatedResults`, `videoDetails.lengthSeconds`, `viewCount`. Parse with care. `lengthSeconds` is `"0"` for live streams.
15. **URL-encoded and URL-safe base64.** Tokens and params from responses contain `%3D`. Tab params use URL-safe base64 (`_`, for example `EgVzaG93c_IGBAoCYgA`). Send server values unchanged; decode only for debugging.
16. **Short playlist ids on channel home** (13 characters, for example `PLPXG0FHDlqWU`). `contentId`, `watchEndpoint.playlistId` and the "View full playlist" `browseId` (`VLPLPXG0FHDlqWU`) all carry the same short id, so there is no longer id to prefer. Use the "View full playlist" `browseId` as given. Not verified that `browse` accepts it (see Open questions).
17. **Members-only is common on channel tabs** (10 of 30 on LTT Videos page 1). Guests cannot play them. Use the "Public" chip or filter on `BADGE_MEMBERS_ONLY`. On search, the members-only badge was never seen; the yt-dlp rule is unverified.
18. **Sort options are unreliable.** Only relevance and view count work. Upload date and rating are ignored. The upload-date filter "this week" returned one 5-year-old outlier.
19. **Index-based selection in `/next` breaks.** The merch shelf is optional, so the comments section moves. Select by key.
20. **`visitorData` is absent** in responses once you send one. Make it `Option`.
21. **`/player` returns `UNPLAYABLE` without a PO token.** Use it only for metadata (duration, ISO date). Do not treat `UNPLAYABLE` as "video unavailable" in the UI.
22. **Not captured:** premieres, upcoming streams, members-only items in search or related, age-restricted videos. Add fixtures when these appear.
23. **Playlist lockup rows differ by context.** Search lockups have `[channel, kind]` in row 0; channel-home lockups have only `["Playlist"]`. A spacer row with no `metadataParts` sits in the middle. Use `metadataParts[]?`.

---

## Appendix: fixture index

Captured on 2026-10-29. The names below are the capture names. The test fixtures in `tests/fixtures/innertube/` use the same names.

- resolve_url: `resolve_url_handle`, `resolve_url_handle_videos`, `resolve_url_playlist`, `resolve_url_missing_handle`
- browse channel: `browse_channel_home`, `browse_channel_videos`, `browse_channel_videos_cont1`, `browse_channel_videos_chip_public`, `browse_channel_videos_with_visitor`, `browse_channel_videos_ctx_visitor`, `browse_channel_shorts`, `browse_channel_streams`, `browse_channel_streams_pctparams`, `browse_channel_streams_lofigirl`, `browse_channel_handle_browseid_err`
- browse playlist: `browse_playlist`, `browse_playlist_cont1`, `browse_playlist_oldclient`, `browse_playlist_long`, `browse_playlist_long_cont1`, `browse_playlist_long_sectioncont`
- next: `next_chapters`, `next_normal`, `next_live`, `next_chapters_related_cont`, `next_normal_minimal_headers`
- player: `player_chapters`, `player_live`, `player_live_visitor`
- search: `search_*` (plain, date, duration, type, sort, combo, chip, spelling, live, error cases), plus `search_live_lofi_helper`

All files end in `.json`. A subset is in `tests/fixtures/innertube/`, with tracking fields removed. The full set was captured on 2026-10-29 and is not in the repository.

---

## Open questions

These claims could not be checked against the fixtures. Each one needs a new live capture.

1. **Short playlist ids.** Does `browse` with `VL<13-char id>` (for example `VLPLPXG0FHDlqWU` from channel home) return the playlist? No fixture.
2. **Members-only in search and related.** No `BADGE_STYLE_TYPE_MEMBERS_ONLY` or "Members only" label appears in any search fixture, including `search_members_only.json`. The rule in V1 comes from yt-dlp only.
3. **Chapterless VOD.** Both VOD fixtures have chapters. The claim "no `DESCRIPTION_CHAPTERS` entry means no chapters" was seen only on live (`next_live.json` has no `markersMap`). Capture a VOD without chapters.
4. **Premieres and upcoming streams.** No fixture contains `upcomingEventData`, "Premieres" or "Scheduled for". Their duration, view and date text are unknown. The text rules in 3.6 may misclassify them.
5. **Thumbnail URLs.** Do the `?sqp=...&rs=...` query strings expire? Can a URL built from the video id (`https://i.ytimg.com/vi/<id>/mqdefault.jpg`) be used when a lockup has no thumbnail? Not testable offline. This spec says to use URLs as given.
6. **Collab channels on channel grids.** Grid lockups give only the joined text "A and B", with no ids. Is there another source on the grid response, or must the app call `/next`?
7. **Continuation pages for Shorts and Live tabs, and for related videos past page 2.** Only the Videos tab has a captured continuation page (`browse_channel_videos_cont1.json`). The same shape is assumed for the other grids.
8. **Comments continuation type.** Table 1.6 lists comments under `continuationItemRenderer`. Only the first token (in `/next`) was checked; no comments response was captured.
9. **`browseEndpoint` in description `commandRuns`.** Not seen in any fixture (only `urlEndpoint` and `watchEndpoint`).
10. **Unverified channel header badge.** The verified badge path in 2.3 was seen only on verified channels. The shape for a channel with no badge is assumed (field missing).
11. **Server-behaviour claims from live tests.** The results in 4.2 and 4.3 (for example "sort by upload date is ignored", "this week returned one outlier") come from comparing live responses. The fixtures support them, but they can change at any time. Re-check them before each release.
12. **`hl` other than `en`.** Not tested. All text rules assume English.
