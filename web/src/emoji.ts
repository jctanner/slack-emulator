// A practical subset of Slack's emoji shortcodes. Unknown names render as
// `:name:` text, which is also what Slack does for missing custom emoji.

const groups: Record<string, [string, string][]> = {
  Smileys: [
    ['grinning', '😀'], ['smiley', '😃'], ['smile', '😄'], ['grin', '😁'], ['laughing', '😆'],
    ['sweat_smile', '😅'], ['joy', '😂'], ['rolling_on_the_floor_laughing', '🤣'], ['slightly_smiling_face', '🙂'],
    ['upside_down_face', '🙃'], ['wink', '😉'], ['blush', '😊'], ['innocent', '😇'], ['heart_eyes', '😍'],
    ['star-struck', '🤩'], ['kissing_heart', '😘'], ['yum', '😋'], ['stuck_out_tongue', '😛'],
    ['stuck_out_tongue_winking_eye', '😜'], ['zany_face', '🤪'], ['money_mouth_face', '🤑'], ['hugging_face', '🤗'],
    ['thinking_face', '🤔'], ['shushing_face', '🤫'], ['zipper_mouth_face', '🤐'], ['raised_eyebrow', '🤨'],
    ['neutral_face', '😐'], ['expressionless', '😑'], ['no_mouth', '😶'], ['smirk', '😏'], ['unamused', '😒'],
    ['face_with_rolling_eyes', '🙄'], ['grimacing', '😬'], ['relieved', '😌'], ['pensive', '😔'], ['sleepy', '😪'],
    ['sleeping', '😴'], ['mask', '😷'], ['nauseated_face', '🤢'], ['hot_face', '🥵'], ['cold_face', '🥶'],
    ['dizzy_face', '😵'], ['exploding_head', '🤯'], ['cowboy_hat_face', '🤠'], ['partying_face', '🥳'],
    ['sunglasses', '😎'], ['nerd_face', '🤓'], ['confused', '😕'], ['worried', '😟'], ['slightly_frowning_face', '🙁'],
    ['open_mouth', '😮'], ['hushed', '😯'], ['astonished', '😲'], ['flushed', '😳'], ['pleading_face', '🥺'],
    ['fearful', '😨'], ['cold_sweat', '😰'], ['cry', '😢'], ['sob', '😭'], ['scream', '😱'], ['confounded', '😖'],
    ['persevere', '😣'], ['disappointed', '😞'], ['sweat', '😓'], ['weary', '😩'], ['tired_face', '😫'],
    ['yawning_face', '🥱'], ['triumph', '😤'], ['rage', '😡'], ['angry', '😠'], ['face_with_symbols_on_mouth', '🤬'],
    ['smiling_imp', '😈'], ['skull', '💀'], ['poop', '💩'], ['clown_face', '🤡'], ['ghost', '👻'], ['alien', '👽'],
    ['robot_face', '🤖'], ['see_no_evil', '🙈'], ['hear_no_evil', '🙉'], ['speak_no_evil', '🙊'],
  ],
  People: [
    ['wave', '👋'], ['raised_hand', '✋'], ['ok_hand', '👌'], ['pinched_fingers', '🤌'], ['v', '✌️'],
    ['crossed_fingers', '🤞'], ['metal', '🤘'], ['call_me_hand', '🤙'], ['point_left', '👈'], ['point_right', '👉'],
    ['point_up', '☝️'], ['point_down', '👇'], ['+1', '👍'], ['-1', '👎'], ['fist', '✊'], ['facepunch', '👊'],
    ['clap', '👏'], ['raised_hands', '🙌'], ['open_hands', '👐'], ['handshake', '🤝'], ['pray', '🙏'],
    ['muscle', '💪'], ['writing_hand', '✍️'], ['eyes', '👀'], ['brain', '🧠'], ['facepalm', '🤦'], ['shrug', '🤷'],
    ['bow', '🙇'], ['man-raising-hand', '🙋‍♂️'], ['woman-raising-hand', '🙋‍♀️'], ['dancer', '💃'],
    ['runner', '🏃'], ['technologist', '🧑‍💻'], ['ninja', '🥷'], ['superhero', '🦸'],
  ],
  Nature: [
    ['dog', '🐶'], ['cat', '🐱'], ['mouse', '🐭'], ['fox_face', '🦊'], ['bear', '🐻'], ['panda_face', '🐼'],
    ['koala', '🐨'], ['tiger', '🐯'], ['lion_face', '🦁'], ['cow', '🐮'], ['pig', '🐷'], ['frog', '🐸'],
    ['monkey_face', '🐵'], ['chicken', '🐔'], ['penguin', '🐧'], ['bird', '🐦'], ['owl', '🦉'], ['unicorn_face', '🦄'],
    ['bee', '🐝'], ['bug', '🐛'], ['butterfly', '🦋'], ['snail', '🐌'], ['turtle', '🐢'], ['snake', '🐍'],
    ['octopus', '🐙'], ['crab', '🦀'], ['tropical_fish', '🐠'], ['whale', '🐳'], ['shark', '🦈'], ['sloth', '🦥'],
    ['cactus', '🌵'], ['evergreen_tree', '🌲'], ['seedling', '🌱'], ['herb', '🌿'], ['four_leaf_clover', '🍀'],
    ['fallen_leaf', '🍂'], ['mushroom', '🍄'], ['sunflower', '🌻'], ['rose', '🌹'], ['tulip', '🌷'],
    ['sunny', '☀️'], ['cloud', '☁️'], ['rainbow', '🌈'], ['snowflake', '❄️'], ['zap', '⚡'], ['fire', '🔥'],
    ['droplet', '💧'], ['ocean', '🌊'], ['star', '⭐'], ['star2', '🌟'], ['sparkles', '✨'], ['crescent_moon', '🌙'],
    ['earth_americas', '🌎'],
  ],
  Food: [
    ['apple', '🍎'], ['banana', '🍌'], ['watermelon', '🍉'], ['grapes', '🍇'], ['strawberry', '🍓'], ['peach', '🍑'],
    ['cherries', '🍒'], ['avocado', '🥑'], ['hot_pepper', '🌶️'], ['corn', '🌽'], ['bread', '🍞'], ['cheese_wedge', '🧀'],
    ['hamburger', '🍔'], ['fries', '🍟'], ['pizza', '🍕'], ['hotdog', '🌭'], ['taco', '🌮'], ['burrito', '🌯'],
    ['ramen', '🍜'], ['sushi', '🍣'], ['popcorn', '🍿'], ['doughnut', '🍩'], ['cookie', '🍪'], ['birthday', '🎂'],
    ['cake', '🍰'], ['chocolate_bar', '🍫'], ['coffee', '☕'], ['tea', '🍵'], ['beer', '🍺'], ['beers', '🍻'],
    ['wine_glass', '🍷'], ['cocktail', '🍸'], ['champagne', '🍾'],
  ],
  Activities: [
    ['soccer', '⚽'], ['ping_pong', '🏓'], ['basketball', '🏀'], ['football', '🏈'], ['baseball', '⚾'], ['tennis', '🎾'],
    ['trophy', '🏆'], ['medal', '🏅'], ['first_place_medal', '🥇'], ['dart', '🎯'], ['video_game', '🎮'],
    ['game_die', '🎲'], ['jigsaw', '🧩'], ['art', '🎨'], ['guitar', '🎸'], ['musical_note', '🎵'],
    ['headphones', '🎧'], ['tada', '🎉'], ['confetti_ball', '🎊'], ['balloon', '🎈'], ['gift', '🎁'],
    ['ticket', '🎫'], ['rocket', '🚀'], ['airplane', '✈️'], ['car', '🚗'], ['bike', '🚲'], ['ship', '🚢'],
    ['house', '🏠'], ['office', '🏢'], ['hospital', '🏥'], ['construction', '🚧'], ['rotating_light', '🚨'],
    ['checkered_flag', '🏁'], ['triangular_flag_on_post', '🚩'],
  ],
  Objects: [
    ['computer', '💻'], ['keyboard', '⌨️'], ['desktop_computer', '🖥️'], ['iphone', '📱'], ['phone', '☎️'],
    ['camera', '📷'], ['tv', '📺'], ['bulb', '💡'], ['flashlight', '🔦'], ['battery', '🔋'], ['electric_plug', '🔌'],
    ['money_with_wings', '💸'], ['moneybag', '💰'], ['credit_card', '💳'], ['gem', '💎'], ['wrench', '🔧'],
    ['hammer', '🔨'], ['hammer_and_wrench', '🛠️'], ['gear', '⚙️'], ['nut_and_bolt', '🔩'], ['link', '🔗'],
    ['lock', '🔒'], ['unlock', '🔓'], ['key', '🔑'], ['shield', '🛡️'], ['package', '📦'], ['email', '📧'],
    ['inbox_tray', '📥'], ['outbox_tray', '📤'], ['memo', '📝'], ['pencil2', '✏️'], ['pushpin', '📌'],
    ['paperclip', '📎'], ['scissors', '✂️'], ['calendar', '📆'], ['date', '📅'], ['clipboard', '📋'],
    ['chart_with_upwards_trend', '📈'], ['chart_with_downwards_trend', '📉'], ['bar_chart', '📊'],
    ['books', '📚'], ['book', '📖'], ['bookmark', '🔖'], ['label', '🏷️'], ['mag', '🔍'], ['microscope', '🔬'],
    ['test_tube', '🧪'], ['dna', '🧬'], ['pill', '💊'], ['bell', '🔔'], ['no_bell', '🔕'], ['loudspeaker', '📢'],
    ['mega', '📣'], ['hourglass', '⌛'], ['stopwatch', '⏱️'], ['alarm_clock', '⏰'], ['crystal_ball', '🔮'],
    ['magic_wand', '🪄'], ['bomb', '💣'], ['coffin', '⚰️'],
  ],
  Symbols: [
    ['heart', '❤️'], ['orange_heart', '🧡'], ['yellow_heart', '💛'], ['green_heart', '💚'], ['blue_heart', '💙'],
    ['purple_heart', '💜'], ['black_heart', '🖤'], ['broken_heart', '💔'], ['sparkling_heart', '💖'],
    ['100', '💯'], ['boom', '💥'], ['dizzy', '💫'], ['speech_balloon', '💬'], ['thought_balloon', '💭'],
    ['zzz', '💤'], ['white_check_mark', '✅'], ['heavy_check_mark', '✔️'], ['ballot_box_with_check', '☑️'],
    ['x', '❌'], ['negative_squared_cross_mark', '❎'], ['heavy_plus_sign', '➕'], ['heavy_minus_sign', '➖'],
    ['question', '❓'], ['grey_question', '❔'], ['exclamation', '❗'], ['bangbang', '‼️'], ['warning', '⚠️'],
    ['no_entry', '⛔'], ['no_entry_sign', '🚫'], ['stop_sign', '🛑'], ['red_circle', '🔴'], ['large_orange_circle', '🟠'],
    ['large_yellow_circle', '🟡'], ['large_green_circle', '🟢'], ['large_blue_circle', '🔵'],
    ['large_purple_circle', '🟣'], ['white_circle', '⚪'], ['black_circle', '⚫'], ['arrow_up', '⬆️'],
    ['arrow_down', '⬇️'], ['arrow_left', '⬅️'], ['arrow_right', '➡️'], ['arrows_counterclockwise', '🔄'],
    ['repeat', '🔁'], ['new', '🆕'], ['free', '🆓'], ['up', '🆙'], ['cool', '🆒'], ['ok', '🆗'], ['sos', '🆘'],
    ['information_source', 'ℹ️'], ['recycle', '♻️'], ['copyright', '©️'], ['registered', '®️'], ['tm', '™️'],
    ['hash', '#️⃣'], ['one', '1️⃣'], ['two', '2️⃣'], ['three', '3️⃣'], ['four', '4️⃣'], ['five', '5️⃣'],
  ],
}

const aliases: Record<string, string> = {
  thumbsup: '+1',
  thumbsdown: '-1',
  slightly_smiling: 'slightly_smiling_face',
  simple_smile: 'slightly_smiling_face',
  thinking: 'thinking_face',
  hugs: 'hugging_face',
  heavy_heart: 'heart',
  hankey: 'poop',
  shit: 'poop',
  satisfied: 'laughing',
  punch: 'facepunch',
  thumbs_up: '+1',
  thumbs_down: '-1',
  check: 'heavy_check_mark',
  ship_it: 'rocket',
  shipit: 'rocket',
  lightning: 'zap',
  flame: 'fire',
  party: 'tada',
  partying: 'partying_face',
  robot: 'robot_face',
  rolling_eyes: 'face_with_rolling_eyes',
  sweat_drops: 'droplet',
  hourglass_flowing_sand: 'hourglass',
  red_heart: 'heart',
  white_square_button: 'ballot_box_with_check',
}

export const EMOJI_GROUPS: { name: string; emoji: { name: string; char: string }[] }[] = Object.entries(groups).map(
  ([name, list]) => ({ name, emoji: list.map(([n, char]) => ({ name: n, char })) }),
)

const byName = new Map<string, string>()
for (const g of EMOJI_GROUPS) for (const e of g.emoji) byName.set(e.name, e.char)

const SKIN_TONES = ['', '🏻', '🏼', '🏽', '🏾', '🏿']

/** Unicode for a shortcode (without colons), honouring `::skin-tone-N`; undefined if unknown. */
export function emojiChar(name: string): string | undefined {
  const [base, mod] = name.split('::')
  const char = byName.get(base) ?? byName.get(aliases[base] ?? '')
  if (!char) return undefined
  const tone = /^skin-tone-([2-6])$/.exec(mod ?? '')
  return tone ? char.replace('️', '') + SKIN_TONES[Number(tone[1]) - 1] : char
}

/** Shortcodes matching a search prefix/substring, best matches first. */
export function searchEmoji(query: string, limit = 50): { name: string; char: string }[] {
  const q = query.toLowerCase()
  const starts: { name: string; char: string }[] = []
  const contains: { name: string; char: string }[] = []
  for (const g of EMOJI_GROUPS) {
    for (const e of g.emoji) {
      if (e.name.startsWith(q)) starts.push(e)
      else if (e.name.includes(q)) contains.push(e)
    }
  }
  for (const [alias, target] of Object.entries(aliases)) {
    if (alias.startsWith(q) && !starts.some((e) => e.name === target)) {
      const char = byName.get(target)
      if (char) starts.push({ name: alias, char })
    }
  }
  return [...starts, ...contains].slice(0, limit)
}
