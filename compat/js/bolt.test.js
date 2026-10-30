// Real Bolt for JavaScript apps against the emulator, over Socket Mode and HTTP.

import assert from 'node:assert/strict'
import { createServer } from 'node:net'
import { test } from 'node:test'
import bolt from '@slack/bolt'
import { API_URL, Person, manifest, waitUntil } from './emu.js'

const { App, LogLevel } = bolt

const TICKET_MODAL = {
  type: 'modal',
  callback_id: 'ticket',
  title: { type: 'plain_text', text: 'New ticket' },
  submit: { type: 'plain_text', text: 'Create' },
  blocks: [
    {
      type: 'input',
      block_id: 'title',
      label: { type: 'plain_text', text: 'Title' },
      element: { type: 'plain_text_input', action_id: 'value' },
    },
  ],
}

function handlers(app, prefix) {
  app.message('ping', async ({ say }) => {
    await say('pong')
  })
  app.message('report please', async ({ message, client }) => {
    await client.files.uploadV2({
      channel_id: message.channel,
      content: 'name,count\nwidgets,3\n',
      filename: 'report.csv',
      title: 'Widget report',
      initial_comment: "Here's the report",
    })
  })
  app.event('app_mention', async ({ event, say }) => {
    await say({ text: `hi <@${event.user}>`, thread_ts: event.ts })
  })
  app.command(`/${prefix}-echo`, async ({ ack, respond, command }) => {
    await ack()
    await respond({ response_type: 'in_channel', text: `echo: ${command.text}` })
  })
  app.command(`/${prefix}-ticket`, async ({ ack, say }) => {
    await ack()
    await say({
      text: 'Need a ticket?',
      blocks: [
        { type: 'section', text: { type: 'mrkdwn', text: 'Need a ticket?' } },
        {
          type: 'actions',
          block_id: 'ticket_actions',
          elements: [{ type: 'button', action_id: 'open_ticket', text: { type: 'plain_text', text: 'Open' } }],
        },
      ],
    })
  })
  app.action('open_ticket', async ({ ack, body, respond, client }) => {
    await ack()
    await respond({ replace_original: true, text: 'Opening a ticket…' })
    await client.views.open({ trigger_id: body.trigger_id, view: { ...TICKET_MODAL, private_metadata: body.channel.id } })
  })
  app.view('ticket', async ({ ack, body, view, client }) => {
    const title = view.state.values.title.value.value
    if (title.length < 5) {
      await ack({ response_action: 'errors', errors: { title: 'Title must be at least 5 characters' } })
      return
    }
    await ack()
    await client.chat.postMessage({ channel: view.private_metadata, text: `<@${body.user.id}> opened: ${title}` })
  })
  app.shortcut('summarize', async ({ ack, shortcut, respond }) => {
    await ack()
    await respond({ response_type: 'in_channel', text: `Summary of: ${shortcut.message.text}` })
  })
  app.shortcut('new_ticket', async ({ ack, body, client }) => {
    await ack()
    await client.views.open({
      trigger_id: body.trigger_id,
      view: {
        type: 'modal',
        callback_id: 'lookup',
        title: { type: 'plain_text', text: 'Find a ticket' },
        submit: { type: 'plain_text', text: 'Open' },
        blocks: [
          {
            type: 'input',
            block_id: 'ticket',
            label: { type: 'plain_text', text: 'Ticket' },
            element: { type: 'external_select', action_id: 'ticket_lookup', min_query_length: 2 },
          },
        ],
      },
    })
  })
  app.options('ticket_lookup', async ({ ack, payload }) => {
    const q = payload.value
    await ack({ options: [1, 2].map((n) => ({ text: { type: 'plain_text', text: `${q}-${n}` }, value: String(n) })) })
  })
  app.event('app_home_opened', async ({ event, client }) => {
    if (event.tab !== 'home') return
    await client.views.publish({
      user_id: event.user,
      view: { type: 'home', blocks: [{ type: 'section', text: { type: 'mrkdwn', text: `Welcome home, <@${event.user}>` } }] },
    })
  })
}

async function exercise(person, botUser, prefix, appId) {
  const general = await person.channelId('general')
  await person.api('conversations.invite', { channel: general, users: botUser })

  await person.api('chat.postMessage', { channel: general, text: 'ping' })
  await person.waitForMessage(general, (m) => m.user === botUser && m.text === 'pong')

  const asked = await person.api('chat.postMessage', { channel: general, text: `<@${botUser}> hello there` })
  const reply = await person.waitForMessage(general, (m) => m.user === botUser && m.text === `hi <@${person.userId}>`, {
    threadTs: asked.ts,
  })
  assert.equal(reply.thread_ts, asked.ts)
  assert.equal(reply.bot_profile.user_id, botUser)

  // files.uploadV2: three steps (get URL, multipart POST, complete), then a file_share message.
  await person.api('chat.postMessage', { channel: general, text: 'report please' })
  const shared = await person.waitForMessage(general, (m) => m.user === botUser && m.files)
  assert.equal(shared.text, "Here's the report")
  const f = shared.files[0]
  assert.deepEqual([f.name, f.title, f.filetype], ['report.csv', 'Widget report', 'csv'])
  assert.equal((await person.download(f.url_private_download)).toString(), 'name,count\nwidgets,3\n')

  // Slash command answered through the response URL.
  await person.api('chat.command', { channel: general, command: `/${prefix}-echo`, text: 'hello' })
  await person.waitForMessage(general, (m) => m.user === botUser && m.text === 'echo: hello')

  // A button opens a modal; the app validates the submission.
  await person.api('chat.command', { channel: general, command: `/${prefix}-ticket`, text: '' })
  const prompt = await person.waitForMessage(general, (m) => m.user === botUser && m.text === 'Need a ticket?')
  await person.api('emu.blocks.action', {
    container: { type: 'message', channel_id: general, message_ts: prompt.ts },
    block_id: 'ticket_actions',
    action_id: 'open_ticket',
  })
  const view = await waitUntil(async () => (await person.api('emu.views.current')).views.at(-1), 'the modal')
  assert.equal(view.callback_id, 'ticket')
  const replaced = await person.waitForMessage(general, (m) => m.ts === prompt.ts && m.text === 'Opening a ticket…')
  assert.equal(replaced.blocks, undefined)

  const state = (title) => ({ values: { title: { value: { type: 'plain_text_input', value: title } } } })
  const rejected = await person.api('emu.views.submit', { view_id: view.id, state: state('hi') })
  assert.deepEqual(rejected.errors, { title: 'Title must be at least 5 characters' })
  await person.api('emu.views.submit', { view_id: view.id, state: state('Broken build') })
  assert.deepEqual((await person.api('emu.views.current')).views, [])
  await person.waitForMessage(general, (m) => m.user === botUser && m.text === `<@${person.userId}> opened: Broken build`)

  // A message shortcut answers through its response URL.
  const long = await person.api('chat.postMessage', { channel: general, text: 'a very long discussion' })
  await person.api('emu.shortcuts.run', { app_id: appId, callback_id: 'summarize', channel: general, message_ts: long.ts })
  await person.waitForMessage(general, (m) => m.text === 'Summary of: a very long discussion')

  // A global shortcut opens a modal whose external select asks the app for options.
  await person.api('emu.shortcuts.run', { app_id: appId, callback_id: 'new_ticket' })
  const lookup = await waitUntil(
    async () => (await person.api('emu.views.current')).views.find((v) => v.callback_id === 'lookup'),
    'the lookup modal',
  )
  const found = await person.api('emu.blocks.suggest', {
    container: { type: 'view', view_id: lookup.id },
    block_id: 'ticket',
    action_id: 'ticket_lookup',
    value: 'bug',
  })
  assert.deepEqual(
    found.options.map((o) => o.text.text),
    ['bug-1', 'bug-2'],
  )
  await person.api('emu.views.close', { view_id: lookup.id, all: true })

  // App Home: opening the tab makes the app publish a view.
  const dm = (await person.api('conversations.open', { users: botUser })).channel.id
  await person.api('emu.apps.home', { channel: dm, tab: 'home' })
  const home = await waitUntil(async () => (await person.api('emu.apps.home', { channel: dm })).view, 'the Home tab')
  assert.equal(home.blocks[0].text.text, `Welcome home, <@${person.userId}>`)
}

function freePort() {
  return new Promise((resolve) => {
    const srv = createServer()
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address()
      srv.close(() => resolve(port))
    })
  })
}

test('socket mode', async () => {
  const alice = await Person.signup('alice')
  const prefix = 'js-sock'
  const spec = manifest('Socket JS', { socketMode: true, commands: [`/${prefix}-echo`, `/${prefix}-ticket`] })
  const { app_id } = await alice.api('apps.manifest.create', { manifest: spec })
  const install = await alice.api('emu.apps.install', { app_id })
  const { token: appToken } = await alice.api('emu.apps.tokens.create', { app_id })

  const app = new App({
    token: install.access_token,
    appToken,
    socketMode: true,
    logLevel: LogLevel.WARN,
    clientOptions: { slackApiUrl: API_URL },
  })
  handlers(app, prefix)
  await app.start()
  try {
    await exercise(alice, install.bot_user_id, prefix, app_id)
  } finally {
    await app.stop()
  }
})

test('http events', async () => {
  const alice = await Person.signup('alice')
  const prefix = 'js-http'
  const port = await freePort()
  const requestUrl = `http://127.0.0.1:${port}/slack/events`
  // Bolt needs the credentials before it can answer url_verification, so
  // verification fails at creation and is retried once Bolt is up.
  const spec = manifest('Http JS', { requestUrl, commands: [`/${prefix}-echo`, `/${prefix}-ticket`] })
  const created = await alice.api('apps.manifest.create', { manifest: spec })
  const install = await alice.api('emu.apps.install', { app_id: created.app_id })

  const app = new App({
    token: install.access_token,
    signingSecret: created.credentials.signing_secret,
    logLevel: LogLevel.WARN,
    clientOptions: { slackApiUrl: API_URL },
  })
  handlers(app, prefix)
  await app.start(port)
  try {
    const verified = await alice.api('emu.apps.verifyUrl', { app_id: created.app_id })
    assert.equal(verified.verified, true, JSON.stringify(verified))
    await exercise(alice, install.bot_user_id, prefix, created.app_id)
  } finally {
    await app.stop()
  }
})
