#!/usr/bin/env python3
"""Второй проход: то, что осталось кириллицей после первого."""
import re

M2 = {
    'println!("  решение не получено: {e}");': 'println!("  no decision came back: {e}");',
    '"неудача: нажатие на {} «{}» не привело ни на какую другую страницу",': '"failed: clicking {} «{}» led to no other page",',
    'println!("проверка по цели: {} — {why}", if ok { "достигнута" } else { "НЕ достигнута" });':
        'println!("goal check: {} — {why}", if ok { "reached" } else { "NOT reached" });',
    'println!("проверка по цели не удалась ({e}) — навык считается непроверенным");':
        'println!("the goal check failed ({e}) — the skill is treated as unverified");',
    'println!("прогон был обрезан на развороте, но цель подтверждена — шаг, который её достиг, сохранён");':
        'println!("the run was cut at a reversal, but the goal is confirmed — the step that reached it is kept");',
    'if sk.learn_declared_done { "да" } else { "НЕТ — навык непроверенный" }':
        'if sk.learn_declared_done { "yes" } else { "NO — the skill is unverified" }',
    'println!("СТАРЫЙ ПУТЬ — модель решает каждый шаг:");': 'println!("THE OLD PATH — the model decides every step:");',
    '"нет — пришли не туда, и это видно, а не замалчивается"': '"no — it landed somewhere else, and that is shown rather than hidden"',
    '"  goal reached: {}"': '"  reached the goal: {}"',
    'println!("  программа исправлена и сохранена: {healed}");': 'println!("  the program was repaired and saved: {healed}");',
    '"  старый путь: {:.1} с, {} обращений к модели → {slow_url}"': '"  the old path: {:.1} s, {} model calls → {slow_url}"',
    'println!("  быстрее в {:.1}x", slow_run.ms as f64 / (r.ms as f64).max(1.0));':
        'println!("  faster by {:.1}x", slow_run.ms as f64 / (r.ms as f64).max(1.0));',
    '"пришли на страницу «{}», ожидали «{}» — {}",': '"landed on page «{}», expected «{}» — {}",',
    'if slow_ok { "совпадает" } else { "НЕ совпадает" }': 'if slow_ok { "matches" } else { "does NOT match" }',
    'if fast_ok { "совпадает" } else { "НЕ совпадает" }': 'if fast_ok { "matches" } else { "does NOT match" }',
    'println!("ОБЫЧНЫЙ АГЕНТ — модель на каждом шаге");': 'println!("ORDINARY AGENT — the model at every step");',
    'println!("ИТОГ ДЛЯ СЦЕНЫ");': 'println!("THE NUMBER FOR THE STAGE");',
    'против {fast_steps}': 'against {fast_steps}',
    'println!("на чём учились: {}", sk.learned_from);': 'println!("learned on: {}", sk.learned_from);',
    '"  программа исправлена и сохранена': '"  the program was repaired and saved',
    'println!("  пришли на: {landing}");': 'println!("  landed on: {landing}");',
    '"да"': '"yes"',
}

R2 = {
    "<title>mint — гонка</title>": "<title>mint — the race</title>",
    "m[1]+' задач подряд. Two panes. Watch the decision counters.';": "m[1]+' tasks in a row. Two panes. Watch the decision counters.';",
    '"Витрина mint: принимает кадры и числа от рантайма, отдаёт их странице."':
        '"mint showcase: takes frames and numbers from the runtime and serves them to the page."',
    "Почему опрос, а не websocket: картинка меняется только на шагах агента, то есть":
        "Why polling and not a websocket: the picture changes only on agent steps, that is",
    "примерно раз в секунду. Опрос раз в 300 мс отдаёт то же самое, но проходит через":
        "about once a second. Polling every 300 ms delivers the same, but goes through any",
    "любой туннель без возни с рукопожатиями и не ломается от буферизации посредника.":
        "tunnel without handshake trouble and does not break on an intermediary's buffering.",
    "Три точки входа:": "Three entry points:",
    "GET  /         — страница для проектора": "GET  /         — the page for the projector",
    "GET  /state    — текущее состояние: числа и кадры обеих сторон": "GET  /state    — current state: numbers and frames for both sides",
    "POST /push     — сюда рантайм присылает шаг": "POST /push     — where the runtime sends a step",
    "# тишина в логах: сюда сыплется опрос раз в 300 мс": "# quiet logs: a poll lands here every 300 ms",
}

H = """mint — an agent runtime that does not pay twice for the same decision.

  mint probe --url URL
      look at a page and report what its perception costs

  mint learn --goal G --start URL --skill FILE [--max-steps N] [--category C]
      solve a task with a model in the loop and write the experience down as a program

  mint race --skill FILE --start URL
      the same task twice: the old path against the compiled one

  mint eval --jobs FILE --out FILE.json [--labels FILE] [--category C]
      run the compiled skills and give a verdict for every task

  mint chain --jobs FILE [--push URL] [--max-steps N]
      many tasks in a row, streamed to the showcase page

variables: MINT_MODEL, MINT_MODEL_URL, MINT_MODEL_API, MINT_API_KEY
"""

for path, mapping in (("src/main.rs", M2), ("demo/relay.py", R2)):
    text = open(path, encoding="utf-8").read()
    hits = 0
    for ru, en in mapping.items():
        if ru in text:
            text = text.replace(ru, en)
            hits += 1
    open(path, "w", encoding="utf-8").write(text)
    print(f"{path}: заменено {hits}")

open("src/help.txt", "w", encoding="utf-8").write(H)
print("src/help.txt: переписан на английском")

print("\n=== осталось кириллицей в исходниках ===")
left = 0
for path in ("src/main.rs", "src/skill.rs", "src/decide.rs", "src/cdp.rs", "src/state.rs",
             "demo/relay.py", "src/help.txt"):
    for i, line in enumerate(open(path, encoding="utf-8"), 1):
        if re.search(r"[А-Яа-яЁё]", line):
            print(f"{path}:{i}: {line.strip()[:100]}")
            left += 1
print(f"строк осталось: {left}")
