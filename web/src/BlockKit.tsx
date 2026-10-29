// Block Kit rendering for messages, modals and App Home: blocks, interactive
// elements (which report back to the app as `block_actions`), and legacy
// message attachments.

import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { ApiError, type Attachment, type Block, type Conversation, type StateValues } from './api'
import { useWs } from './context'
import { emojiChar } from './emoji'
import { Mrkdwn, displayName, type MrkdwnContext } from './mrkdwn'
import { convName, useWorkspace } from './store'

export type Container =
  | { type: 'message'; channel_id: string; message_ts: string; is_ephemeral?: boolean }
  | { type: 'view'; view_id: string }

type StateValue = { type: string; [key: string]: any }

/** The value an element starts with (`initial_*`), for stateful elements. */
function initialValue(el: Block, channel?: string): StateValue | undefined {
  const t = el.type
  switch (t) {
    case 'plain_text_input':
    case 'email_text_input':
    case 'url_text_input':
    case 'number_input':
      return { type: t, value: el.initial_value ?? null }
    case 'rich_text_input':
      return { type: t, rich_text_value: el.initial_value ?? null }
    case 'static_select':
    case 'external_select':
    case 'radio_buttons':
      return { type: t, selected_option: el.initial_option ?? null }
    case 'multi_static_select':
    case 'multi_external_select':
    case 'checkboxes':
      return { type: t, selected_options: el.initial_options ?? [] }
    case 'users_select':
      return { type: t, selected_user: el.initial_user ?? null }
    case 'multi_users_select':
      return { type: t, selected_users: el.initial_users ?? [] }
    case 'conversations_select':
      return {
        type: t,
        selected_conversation: el.initial_conversation ?? (el.default_to_current_conversation ? channel : null) ?? null,
      }
    case 'multi_conversations_select':
      return {
        type: t,
        selected_conversations: el.initial_conversations ?? (el.default_to_current_conversation && channel ? [channel] : []),
      }
    case 'channels_select':
      return { type: t, selected_channel: el.initial_channel ?? null }
    case 'multi_channels_select':
      return { type: t, selected_channels: el.initial_channels ?? [] }
    case 'datepicker':
      return { type: t, selected_date: el.initial_date ?? null }
    case 'timepicker':
      return { type: t, selected_time: el.initial_time ?? null }
    case 'datetimepicker':
      return { type: t, selected_date_time: el.initial_date_time ?? null }
  }
  return undefined
}

/** A block's interactive elements. */
function elementsOf(b: Block): Block[] {
  if (b.type === 'section') return b.accessory ? [b.accessory] : []
  if (b.type === 'actions') return b.elements ?? []
  if (b.type === 'input') return b.element ? [b.element] : []
  return []
}

/** State for these blocks: values already entered (for elements that still exist), else initial values. */
function initialState(blocks: Block[], prev: StateValues, channel?: string): StateValues {
  const out: StateValues = {}
  for (const b of blocks) {
    if (!b.block_id) continue
    for (const el of elementsOf(b)) {
      const kept = prev[b.block_id]?.[el.action_id]
      const v = kept && kept.type === el.type ? kept : initialValue(el, channel)
      if (v) (out[b.block_id] ??= {})[el.action_id] = v
    }
  }
  return out
}

function actionError(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.code === 'dispatch_failed') return `The app didn’t respond (${e.data.detail ?? 'no answer'}).`
    if (e.code === 'interactivity_disabled') return 'This app hasn’t turned on interactivity in its manifest.'
    if (e.code === 'app_not_installed') return 'This app isn’t installed any more.'
    return `Couldn’t send that to the app (${e.code}).`
  }
  return 'Couldn’t reach the server.'
}

/** State and action dispatch for one surface (a message, modal or Home tab). */
export function useBlockSurface(blocks: Block[], container: Container, seed?: StateValues) {
  const { store } = useWs()
  const channel = container.type === 'message' ? container.channel_id : undefined
  const [values, setValues] = useState<StateValues>(() => initialState(blocks, seed ?? {}, channel))
  const [busy, setBusy] = useState<string | null>(null)
  const [failure, setFailure] = useState<{ blockId: string; message: string } | null>(null)
  const latest = useRef(values)
  latest.current = values

  // New blocks (message edited, view updated): keep what still applies.
  const signature = useMemo(() => JSON.stringify(blocks), [blocks])
  const firstRun = useRef(true)
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false
      return
    }
    setValues((v) => initialState(blocks, v, channel))
  }, [signature])

  const setValue = (blockId: string, actionId: string, v: StateValue) => {
    latest.current = { ...latest.current, [blockId]: { ...latest.current[blockId], [actionId]: v } }
    setValues(latest.current)
  }

  const dispatch = async (blockId: string, el: Block, value?: StateValue) => {
    const key = `${blockId}/${el.action_id}`
    setBusy(key)
    setFailure(null)
    try {
      await store.call('emu.blocks.action', {
        container,
        block_id: blockId,
        action_id: el.action_id,
        value,
        state: { values: latest.current },
      })
    } catch (e) {
      setFailure({ blockId, message: actionError(e) })
    } finally {
      setBusy((b) => (b === key ? null : b))
    }
  }

  return { values, setValue, dispatch, busy, failure, container }
}

export type Surface = ReturnType<typeof useBlockSurface> & {
  mctx: MrkdwnContext
  /** block_id → validation message (modals). */
  errors?: Record<string, string>
}

function emojify(text: string): string {
  return text.replace(/:([a-z0-9_+'-]+(?:::skin-tone-[2-6])?):/g, (all, name: string) => emojiChar(name) ?? all)
}

/** Plain string for a text object (for `<option>`s and labels). */
export function plain(t?: Block | null): string {
  if (!t?.text) return ''
  return t.type === 'plain_text' && t.emoji !== false ? emojify(t.text) : t.text
}

export function TextObj({ t, ctx }: { t?: Block | null; ctx: MrkdwnContext }) {
  if (!t) return null
  if (t.type === 'mrkdwn') return <Mrkdwn text={t.text ?? ''} ctx={ctx} />
  return <span className="bk-plain">{plain(t)}</span>
}

export function Blocks({ blocks, surface }: { blocks: Block[]; surface: Surface }) {
  return (
    <div className="bk">
      {blocks.map((b, i) => (
        <BlockView key={b.block_id ?? i} block={b} s={surface} />
      ))}
    </div>
  )
}

function BlockView({ block, s }: { block: Block; s: Surface }) {
  const ctx = s.mctx
  const id = block.block_id ?? ''
  let body: ReactNode = null
  switch (block.type) {
    case 'section':
      body = (
        <div className="bk-section">
          <div className="bk-section-main">
            {block.text && <TextObj t={block.text} ctx={ctx} />}
            {block.fields && (
              <div className="bk-fields">
                {block.fields.map((f: Block, i: number) => (
                  <div key={i}>
                    <TextObj t={f} ctx={ctx} />
                  </div>
                ))}
              </div>
            )}
          </div>
          {block.accessory && (
            <div className="bk-accessory">
              {block.accessory.type === 'image' ? (
                <img className="bk-thumb" src={block.accessory.image_url} alt={block.accessory.alt_text} loading="lazy" />
              ) : (
                <Element el={block.accessory} blockId={id} s={s} dispatchOnChange />
              )}
            </div>
          )}
        </div>
      )
      break
    case 'divider':
      body = <hr className="bk-divider" />
      break
    case 'header':
      body = (
        <div className="bk-header" role="heading" aria-level={3}>
          {plain(block.text)}
        </div>
      )
      break
    case 'image':
      body = (
        <figure className="bk-figure">
          {block.title && <figcaption className="bk-image-title">{plain(block.title)}</figcaption>}
          <img className="bk-image" src={block.image_url} alt={block.alt_text} loading="lazy" />
        </figure>
      )
      break
    case 'context':
      body = (
        <div className="bk-context">
          {(block.elements ?? []).map((e: Block, i: number) =>
            e.type === 'image' ? (
              <img key={i} className="bk-context-img" src={e.image_url} alt={e.alt_text} />
            ) : (
              <TextObj key={i} t={e} ctx={ctx} />
            ),
          )}
        </div>
      )
      break
    case 'actions':
      body = (
        <div className="bk-actions">
          {(block.elements ?? []).map((el: Block) => (
            <Element key={el.action_id} el={el} blockId={id} s={s} dispatchOnChange />
          ))}
        </div>
      )
      break
    case 'input':
      body = (
        <div className="bk-input">
          <div className="bk-label">
            {plain(block.label)}
            {block.optional && <span className="muted"> (optional)</span>}
          </div>
          {block.element && <Element el={block.element} blockId={id} s={s} dispatchOnChange={!!block.dispatch_action} />}
          {block.hint && <div className="bk-hint">{plain(block.hint)}</div>}
        </div>
      )
      break
    case 'rich_text':
      body = <RichText elements={block.elements ?? []} ctx={ctx} />
      break
    case 'markdown':
      body = <Mrkdwn text={markdownToMrkdwn(block.text ?? '')} ctx={ctx} />
      break
    case 'video':
      body = (
        <a className="bk-video" href={block.title_url ?? block.video_url} target="_blank" rel="noreferrer noopener">
          <img src={block.thumbnail_url} alt={block.alt_text} loading="lazy" />
          <span>▶︎ {plain(block.title)}</span>
        </a>
      )
      break
    case 'table':
      body = <Table block={block} ctx={ctx} />
      break
    case 'file':
      body = <div className="bk-unsupported">File: {block.external_id}</div>
      break
    default:
      body = <div className="bk-unsupported">This block type ({block.type}) isn’t shown in slack-emu.</div>
  }
  const error = s.errors?.[id]
  const failed = s.failure?.blockId === id ? s.failure.message : null
  return (
    <div className={`bk-block bk-block-${block.type}`}>
      {body}
      {error && <div className="bk-error">{error}</div>}
      {failed && <div className="bk-warning">⚠︎ {failed}</div>}
    </div>
  )
}

/** Converts the `markdown` block's standard Markdown to roughly equivalent mrkdwn. */
function markdownToMrkdwn(md: string): string {
  return md
    .replace(/^#{1,6}\s+(.*)$/gm, '*$1*')
    .replace(/\*\*(.+?)\*\*/g, '*$1*')
    .replace(/__(.+?)__/g, '*$1*')
    .replace(/~~(.+?)~~/g, '~$1~')
    .replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<$2|$1>')
}

function Table({ block, ctx }: { block: Block; ctx: MrkdwnContext }) {
  const rows: Block[][] = block.rows ?? []
  const cell = (c: Block) =>
    c.type === 'rich_text' ? <RichText elements={c.elements ?? []} ctx={ctx} /> : (c.text ?? '')
  return (
    <div className="bk-table-wrap">
      <table className="bk-table">
        <tbody>
          {rows.map((row, r) => (
            <tr key={r}>
              {row.map((c, i) => (r === 0 ? <th key={i}>{cell(c)}</th> : <td key={i}>{cell(c)}</td>))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// ---------- rich_text ----------

function RichText({ elements, ctx }: { elements: Block[]; ctx: MrkdwnContext }) {
  return (
    <div className="bk-rich">
      {elements.map((e, i) => (
        <RichBlock key={i} e={e} ctx={ctx} />
      ))}
    </div>
  )
}

function RichBlock({ e, ctx }: { e: Block; ctx: MrkdwnContext }) {
  const inline = (els: Block[] = []) => els.map((x, i) => <RichInline key={i} e={x} ctx={ctx} />)
  switch (e.type) {
    case 'rich_text_section':
      return <div className="bk-rich-section">{inline(e.elements)}</div>
    case 'rich_text_preformatted':
      return <pre>{inline(e.elements)}</pre>
    case 'rich_text_quote':
      return <blockquote>{inline(e.elements)}</blockquote>
    case 'rich_text_list': {
      const items = (e.elements ?? []).map((item: Block, i: number) => <li key={i}>{inline(item.elements)}</li>)
      const style = { marginLeft: (e.indent ?? 0) * 20 }
      return e.style === 'ordered' ? (
        <ol style={style} start={(e.offset ?? 0) + 1}>
          {items}
        </ol>
      ) : (
        <ul style={style}>{items}</ul>
      )
    }
  }
  return null
}

function RichInline({ e, ctx }: { e: Block; ctx: MrkdwnContext }) {
  let node: ReactNode
  switch (e.type) {
    case 'text':
      node = e.text
      break
    case 'link':
      node = (
        <a href={e.url} target="_blank" rel="noreferrer noopener">
          {e.text || e.url}
        </a>
      )
      break
    case 'user':
      node = <span className={`mention ${e.user_id === ctx.me ? 'me' : ''}`}>@{displayName(ctx.users[e.user_id], e.user_id)}</span>
      break
    case 'channel':
      node = (
        <button type="button" className="channel-link" onClick={() => ctx.onChannel?.(e.channel_id)}>
          #{ctx.convs[e.channel_id]?.name ?? e.channel_id}
        </button>
      )
      break
    case 'usergroup':
      node = <span className="mention">@{e.usergroup_id}</span>
      break
    case 'broadcast':
      node = <span className="mention me">@{e.range}</span>
      break
    case 'emoji':
      node = e.unicode ? String.fromCodePoint(...e.unicode.split('-').map((h: string) => parseInt(h, 16))) : (emojiChar(e.name) ?? `:${e.name}:`)
      break
    case 'date':
      node = e.fallback ?? new Date(e.timestamp * 1000).toLocaleString()
      break
    case 'color':
      node = (
        <span className="bk-color">
          <span style={{ background: e.value }} /> {e.value}
        </span>
      )
      break
    default:
      node = e.text ?? null
  }
  const st = e.style ?? {}
  if (st.code) node = <code>{node}</code>
  if (st.bold) node = <strong>{node}</strong>
  if (st.italic) node = <em>{node}</em>
  if (st.strike) node = <s>{node}</s>
  return <>{node}</>
}

// ---------- elements ----------

function Element({ el, blockId, s, dispatchOnChange }: { el: Block; blockId: string; s: Surface; dispatchOnChange: boolean }) {
  const [confirming, setConfirming] = useState<(() => void) | null>(null)
  const value = s.values[blockId]?.[el.action_id]
  const busy = s.busy === `${blockId}/${el.action_id}`

  const withConfirm = (fn: () => void) => (el.confirm ? setConfirming(() => fn) : fn())
  const change = (v: StateValue) => {
    s.setValue(blockId, el.action_id, v)
    if (dispatchOnChange) withConfirm(() => s.dispatch(blockId, el, v))
  }
  const props = { el, value, change, s, blockId, busy }

  let node: ReactNode
  switch (el.type) {
    case 'button':
    case 'workflow_button':
      node = (
        <button
          type="button"
          className={`bk-button ${el.style ? `bk-${el.style}` : ''} ${busy ? 'busy' : ''}`}
          disabled={el.type === 'workflow_button'}
          aria-busy={busy}
          onClick={() =>
            withConfirm(() => {
              if (el.url) window.open(el.url, '_blank', 'noopener')
              s.dispatch(blockId, el)
            })
          }
        >
          {plain(el.text)}
        </button>
      )
      break
    case 'static_select':
    case 'external_select':
    case 'users_select':
    case 'conversations_select':
    case 'channels_select':
      node = <SingleSelect {...props} />
      break
    case 'multi_static_select':
    case 'multi_external_select':
    case 'multi_users_select':
    case 'multi_conversations_select':
    case 'multi_channels_select':
      node = <MultiSelect {...props} />
      break
    case 'overflow':
      node = (
        <Overflow
          el={el}
          onPick={(o) =>
            withConfirm(() => {
              if (o.url) window.open(o.url, '_blank', 'noopener')
              s.dispatch(blockId, el, { type: 'overflow', selected_option: o })
            })
          }
        />
      )
      break
    case 'checkboxes':
    case 'radio_buttons':
      node = <Choices {...props} />
      break
    case 'datepicker':
      node = (
        <input
          type="date"
          className="bk-field bk-picker"
          value={value?.selected_date ?? ''}
          onChange={(e) => change({ type: el.type, selected_date: e.target.value || null })}
        />
      )
      break
    case 'timepicker':
      node = (
        <input
          type="time"
          className="bk-field bk-picker"
          value={value?.selected_time ?? ''}
          onChange={(e) => change({ type: el.type, selected_time: e.target.value || null })}
        />
      )
      break
    case 'datetimepicker':
      node = (
        <input
          type="datetime-local"
          className="bk-field bk-picker"
          value={value?.selected_date_time ? toLocalInput(value.selected_date_time) : ''}
          onChange={(e) =>
            change({
              type: el.type,
              selected_date_time: e.target.value ? Math.floor(new Date(e.target.value).getTime() / 1000) : null,
            })
          }
        />
      )
      break
    case 'plain_text_input':
    case 'email_text_input':
    case 'url_text_input':
    case 'number_input':
    case 'rich_text_input':
      node = <TextInput {...props} dispatchOnChange={dispatchOnChange} />
      break
    case 'image':
      node = <img className="bk-thumb" src={el.image_url} alt={el.alt_text} />
      break
    case 'file_input':
      node = <div className="bk-unsupported">File uploads aren’t supported in slack-emu yet.</div>
      break
    default:
      node = <div className="bk-unsupported">Unsupported element: {el.type}</div>
  }
  return (
    <>
      {node}
      {confirming && (
        <Confirm
          dialog={el.confirm}
          ctx={s.mctx}
          onDone={(ok) => {
            const fn = confirming
            setConfirming(null)
            if (ok) fn()
          }}
        />
      )}
    </>
  )
}

function toLocalInput(unix: number) {
  const d = new Date(unix * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

type ElementProps = {
  el: Block
  value?: StateValue
  change: (v: StateValue) => void
  s: Surface
  blockId: string
  busy: boolean
}

type Item = { value: string; label: string; group?: string; option?: Block }

/** Choices for a select: its options, or people / conversations from the workspace. */
function useItems(el: Block): Item[] {
  const { store } = useWs()
  const users = useWorkspace(store, (s) => s.users)
  const convs = useWorkspace(store, (s) => s.convs)
  return useMemo(() => {
    const base = el.type.replace(/^multi_/, '')
    if (base === 'static_select') {
      const fromOpts = (opts: Block[] = [], group?: string) =>
        opts.map((o) => ({ value: o.value, label: plain(o.text), group, option: o }))
      return el.option_groups
        ? el.option_groups.flatMap((g: Block) => fromOpts(g.options, plain(g.label)))
        : fromOpts(el.options)
    }
    if (base === 'users_select') {
      return Object.values(users)
        .filter((u) => !u.deleted)
        .map((u) => ({ value: u.id, label: displayName(u) }))
        .sort((a, b) => a.label.localeCompare(b.label))
    }
    if (base === 'conversations_select' || base === 'channels_select') {
      const include: string[] | undefined = base === 'channels_select' ? ['public'] : el.filter?.include
      const kind = (c: Conversation) => (c.is_im ? 'im' : c.is_mpim ? 'mpim' : c.is_private ? 'private' : 'public')
      return Object.values(convs)
        .filter((c) => !c.is_archived && (!include || include.includes(kind(c))))
        .filter((c) => !(el.filter?.exclude_bot_users && c.is_im && users[c.user ?? '']?.is_bot))
        .map((c) => ({
          value: c.id,
          label: (c.is_im || c.is_mpim ? '' : c.is_private ? '🔒︎ ' : '#') + convName(c, { users, me: store.state.me }),
        }))
        .sort((a, b) => a.label.localeCompare(b.label))
    }
    return []
  }, [el, users, convs, store])
}

/** The state key holding a select's value(s). */
function selectKey(type: string): string {
  const base = type.replace(/^multi_/, '')
  const single =
    base === 'users_select'
      ? 'selected_user'
      : base === 'conversations_select'
        ? 'selected_conversation'
        : base === 'channels_select'
          ? 'selected_channel'
          : 'selected_option'
  return type.startsWith('multi_') ? `${single}s` : single
}

function SingleSelect({ el, value, change, busy }: ElementProps) {
  const items = useItems(el)
  const key = selectKey(el.type)
  const isOption = key === 'selected_option'
  const current = isOption ? (value?.[key]?.value ?? '') : (value?.[key] ?? '')
  if (el.type === 'external_select') {
    return (
      <select className="bk-field bk-select" disabled title="External option sources aren’t supported in slack-emu yet">
        <option>{plain(el.placeholder) || 'Select an item'}</option>
      </select>
    )
  }
  const groups = [...new Set(items.map((i) => i.group))]
  const option = (i: Item) => (
    <option key={i.value} value={i.value}>
      {i.label}
    </option>
  )
  return (
    <select
      className="bk-field bk-select"
      value={current}
      aria-busy={busy}
      onChange={(e) => {
        const item = items.find((i) => i.value === e.target.value)
        change({ type: el.type, [key]: isOption ? (item?.option ?? null) : e.target.value || null })
      }}
    >
      <option value="" disabled>
        {plain(el.placeholder) || 'Select an item'}
      </option>
      {groups[0] === undefined
        ? items.map(option)
        : groups.map((g) => (
            <optgroup key={g} label={g}>
              {items.filter((i) => i.group === g).map(option)}
            </optgroup>
          ))}
    </select>
  )
}

function MultiSelect({ el, value, change }: ElementProps) {
  const items = useItems(el)
  const key = selectKey(el.type)
  const isOption = key === 'selected_options'
  const selected: string[] = (value?.[key] ?? []).map((v: any) => (isOption ? v.value : v))
  const set = (vals: string[]) =>
    change({
      type: el.type,
      [key]: isOption ? vals.map((v) => items.find((i) => i.value === v)?.option).filter(Boolean) : vals,
    })
  const full = el.max_selected_items && selected.length >= el.max_selected_items
  return (
    <div className="bk-multi">
      {selected.map((v) => (
        <span key={v} className="bk-chip">
          {items.find((i) => i.value === v)?.label ?? v}
          <button type="button" aria-label="Remove" onClick={() => set(selected.filter((x) => x !== v))}>
            ×
          </button>
        </span>
      ))}
      {el.type === 'multi_external_select' ? (
        <span className="bk-unsupported">External option sources aren’t supported yet.</span>
      ) : (
        !full && (
          <select className="bk-field bk-select" value="" onChange={(e) => e.target.value && set([...selected, e.target.value])}>
            <option value="">{plain(el.placeholder) || 'Select items'}</option>
            {items
              .filter((i) => !selected.includes(i.value))
              .map((i) => (
                <option key={i.value} value={i.value}>
                  {i.group ? `${i.group}: ${i.label}` : i.label}
                </option>
              ))}
          </select>
        )
      )}
    </div>
  )
}

function Choices({ el, value, change, s }: ElementProps) {
  const name = useId()
  const multi = el.type === 'checkboxes'
  const selected: Block[] = multi ? (value?.selected_options ?? []) : value?.selected_option ? [value.selected_option] : []
  const isOn = (o: Block) => selected.some((x) => x.value === o.value)
  return (
    <div className="bk-choices" role={multi ? 'group' : 'radiogroup'}>
      {(el.options ?? []).map((o: Block) => (
        <label key={o.value} className="bk-choice">
          <input
            type={multi ? 'checkbox' : 'radio'}
            name={name}
            checked={isOn(o)}
            onChange={(e) =>
              multi
                ? change({
                    type: el.type,
                    selected_options: e.target.checked ? [...selected, o] : selected.filter((x) => x.value !== o.value),
                  })
                : change({ type: el.type, selected_option: o })
            }
          />
          <span>
            <TextObj t={o.text} ctx={s.mctx} />
            {o.description && (
              <span className="bk-choice-desc">
                <TextObj t={o.description} ctx={s.mctx} />
              </span>
            )}
          </span>
        </label>
      ))}
    </div>
  )
}

function TextInput({ el, value, s, blockId, dispatchOnChange }: ElementProps & { dispatchOnChange: boolean }) {
  const rich = el.type === 'rich_text_input'
  const text: string = rich ? richToText(value?.rich_text_value) : (value?.value ?? '')
  const toValue = (t: string): StateValue =>
    rich
      ? { type: el.type, rich_text_value: t ? textToRich(t) : null }
      : { type: el.type, value: t === '' ? null : t }
  const onEnter = el.dispatch_action_config?.trigger_actions_on ?? ['on_enter_pressed']
  const timer = useRef<number | undefined>(undefined)

  const set = (t: string) => {
    const v = toValue(t)
    s.setValue(blockId, el.action_id, v)
    if (dispatchOnChange && onEnter.includes('on_character_entered')) {
      clearTimeout(timer.current)
      timer.current = window.setTimeout(() => s.dispatch(blockId, el, v), 400)
    }
  }
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
    const submits = !el.multiline || e.metaKey || e.ctrlKey
    if (e.key === 'Enter' && submits && dispatchOnChange && onEnter.includes('on_enter_pressed')) {
      e.preventDefault()
      s.dispatch(blockId, el, toValue(text))
    }
  }
  const common = {
    className: 'bk-field',
    value: text,
    placeholder: plain(el.placeholder),
    autoFocus: !!el.focus_on_load,
    minLength: el.min_length,
    maxLength: el.max_length,
    onKeyDown,
  }
  if (el.multiline || rich) {
    return <textarea {...common} rows={4} onChange={(e) => set(e.target.value)} />
  }
  const type =
    el.type === 'email_text_input' ? 'email' : el.type === 'url_text_input' ? 'url' : el.type === 'number_input' ? 'number' : 'text'
  return (
    <input
      {...common}
      type={type}
      step={el.type === 'number_input' ? (el.is_decimal_allowed ? 'any' : 1) : undefined}
      min={el.min_value}
      max={el.max_value}
      onChange={(e) => set(e.target.value)}
    />
  )
}

function textToRich(t: string): Block {
  return { type: 'rich_text', elements: [{ type: 'rich_text_section', elements: [{ type: 'text', text: t }] }] }
}

function richToText(v?: Block | null): string {
  if (!v) return ''
  return (v.elements ?? [])
    .map((sec: Block) => (sec.elements ?? []).map((e: Block) => e.text ?? '').join(''))
    .join('\n')
}

function Overflow({ el, onPick }: { el: Block; onPick: (o: Block) => void }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false)
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [open])
  return (
    <div className="bk-overflow" ref={ref}>
      <button type="button" className="bk-button bk-more" aria-label="More options" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        ⋯
      </button>
      {open && (
        <ul className="bk-menu" role="menu">
          {(el.options ?? []).map((o: Block) => (
            <li key={o.value}>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setOpen(false)
                  onPick(o)
                }}
              >
                {plain(o.text)}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

/** A confirmation dialog (`confirm` on an element). */
function Confirm({ dialog, ctx, onDone }: { dialog: Block; ctx: MrkdwnContext; onDone: (ok: boolean) => void }) {
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => e.key === 'Escape' && onDone(false)
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [onDone])
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onDone(false)}>
      <div className="card modal bk-confirm" role="alertdialog" aria-modal="true">
        <h1>{plain(dialog.title) || 'Are you sure?'}</h1>
        <TextObj t={dialog.text} ctx={ctx} />
        <div className="bk-modal-actions">
          <button type="button" className="bk-button" onClick={() => onDone(false)}>
            {plain(dialog.deny) || 'Cancel'}
          </button>
          <button type="button" className={`bk-button ${dialog.style === 'danger' ? 'bk-danger' : 'bk-primary'}`} autoFocus onClick={() => onDone(true)}>
            {plain(dialog.confirm) || 'Yes'}
          </button>
        </div>
      </div>
    </div>
  )
}

// ---------- attachments ----------

const ATTACHMENT_COLORS: Record<string, string> = { good: '#2eb886', warning: '#daa038', danger: '#a30200' }

function attachmentColor(c?: string) {
  if (!c) return undefined
  return ATTACHMENT_COLORS[c] ?? (/^[0-9a-f]{3,8}$/i.test(c) ? `#${c}` : c)
}

export function Attachments({ list, surface }: { list: Attachment[]; surface: Surface }) {
  const ctx = surface.mctx
  return (
    <div className="bk-attachments">
      {list.map((a, i) => {
        const hasBody = a.author_name || a.title || a.text || a.fields?.length || a.image_url || a.blocks?.length || a.footer
        return (
          <div key={i}>
            {a.pretext && <Mrkdwn text={a.pretext} ctx={ctx} />}
            <div className="bk-attachment" style={{ borderLeftColor: attachmentColor(a.color) }}>
              <div className="bk-attachment-main">
                {a.author_name && (
                  <div className="bk-att-author">
                    {a.author_icon && <img src={a.author_icon} alt="" />}
                    {a.author_link ? (
                      <a href={a.author_link} target="_blank" rel="noreferrer noopener">
                        {a.author_name}
                      </a>
                    ) : (
                      a.author_name
                    )}
                  </div>
                )}
                {a.title && (
                  <div className="bk-att-title">
                    {a.title_link ? (
                      <a href={a.title_link} target="_blank" rel="noreferrer noopener">
                        {a.title}
                      </a>
                    ) : (
                      a.title
                    )}
                  </div>
                )}
                {a.text && <Mrkdwn text={a.text} ctx={ctx} />}
                {!!a.fields?.length && (
                  <div className="bk-att-fields">
                    {a.fields.map((f, j) => (
                      <div key={j} className={f.short ? 'short' : ''}>
                        {f.title && <div className="bk-att-field-title">{f.title}</div>}
                        {f.value && <Mrkdwn text={f.value} ctx={ctx} />}
                      </div>
                    ))}
                  </div>
                )}
                {a.image_url && <img className="bk-image" src={a.image_url} alt="" loading="lazy" />}
                {!!a.blocks?.length && <Blocks blocks={a.blocks} surface={surface} />}
                {a.footer && (
                  <div className="bk-att-footer">
                    {a.footer_icon && <img src={a.footer_icon} alt="" />}
                    {a.footer}
                    {a.ts && <> · {new Date(Number(a.ts) * 1000).toLocaleString()}</>}
                  </div>
                )}
                {!hasBody && a.fallback && <span className="muted">{a.fallback}</span>}
              </div>
              {a.thumb_url && <img className="bk-thumb" src={a.thumb_url} alt="" />}
            </div>
          </div>
        )
      })}
    </div>
  )
}

/** A message's blocks and attachments, interactive. */
export function MessageBlocks({
  blocks,
  attachments,
  container,
  ctx,
}: {
  blocks?: Block[]
  attachments?: Attachment[]
  container: Container
  ctx: MrkdwnContext
}) {
  const all = useMemo(() => [...(blocks ?? []), ...(attachments ?? []).flatMap((a) => a.blocks ?? [])], [blocks, attachments])
  const s = useBlockSurface(all, container)
  const surface: Surface = { ...s, mctx: ctx }
  return (
    <>
      {!!blocks?.length && <Blocks blocks={blocks} surface={surface} />}
      {!!attachments?.length && <Attachments list={attachments} surface={surface} />}
    </>
  )
}

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/

/** Checks a modal's inputs the way Slack's client does before submitting. */
export function validateInputs(blocks: Block[], values: StateValues): Record<string, string> {
  const errors: Record<string, string> = {}
  for (const b of blocks) {
    if (b.type !== 'input' || !b.block_id || !b.element) continue
    const el = b.element
    const v = values[b.block_id]?.[el.action_id]
    const filled = !!v && Object.entries(v).some(([k, x]) => k !== 'type' && x !== null && x !== '' && !(Array.isArray(x) && x.length === 0))
    if (!filled) {
      if (!b.optional) errors[b.block_id] = 'Please complete this required field.'
      continue
    }
    const text: string | undefined = typeof v.value === 'string' ? v.value : undefined
    if (text === undefined) continue
    if (el.min_length && text.length < el.min_length) errors[b.block_id] = `Must be at least ${el.min_length} characters.`
    else if (el.max_length && text.length > el.max_length) errors[b.block_id] = `Must be ${el.max_length} characters or fewer.`
    else if (el.type === 'email_text_input' && !EMAIL.test(text)) errors[b.block_id] = 'Enter a valid email address.'
    else if (el.type === 'url_text_input' && !/^https?:\/\/\S+$/.test(text)) errors[b.block_id] = 'Enter a valid URL.'
    else if (el.type === 'number_input') {
      const n = Number(text)
      if (!Number.isFinite(n) || (!el.is_decimal_allowed && !Number.isInteger(n))) errors[b.block_id] = 'Enter a valid number.'
      else if (el.min_value !== undefined && n < Number(el.min_value)) errors[b.block_id] = `Must be at least ${el.min_value}.`
      else if (el.max_value !== undefined && n > Number(el.max_value)) errors[b.block_id] = `Must be at most ${el.max_value}.`
    }
  }
  return errors
}
