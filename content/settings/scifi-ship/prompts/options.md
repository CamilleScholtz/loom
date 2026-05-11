The engine has decided the mechanically available actions. Your job is to write the *text* of each option in the protagonist's voice.

<protagonist>
  <name>{{protagonist.name}}</name>
  <background>{{protagonist.background}}</background>
  <traits>{{protagonist.traits}}</traits>
  <mood>{{protagonist.mood}}</mood>
</protagonist>

<scene>
  <where>{{scene.where}}</where>
  <time>{{scene.time}}</time>
</scene>

<actions>
{{actions}}
</actions>

<instructions>
For each action above, write a single short line — a flash of inner thought or stated intent. First person. Working-crew register — plainspoken, technical, the slang of someone who's run this corridor a hundred times. The line should sound like the protagonist's own self-narration, not a menu label. Keep each line under one short sentence; one clause is often enough. Do not invent new actions. Do not skip any. Preserve the order.
</instructions>

<output_format>
Call the `phrase_options` tool with an array of `lines`. The array MUST contain exactly one entry per action, in the same order as the `<actions>` block above.
</output_format>
