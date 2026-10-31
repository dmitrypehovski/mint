#!/usr/bin/env python3
"""Третий проход: внутренние скрипты. Отдельно важна предполётная проверка — она искала
русскую строку итога, а рантайм печатает английскую, и без правки она врала бы."""
import re

M = {
 # demo/run.sh
 "# Прогреть витрину перед выходом на сцену.": "# Warm the showcase up before going on stage.",
 "# Запускает прогон, чтобы панели и счётчики были заполнены, и проверяет, что":
   "# Runs the demo so the panes and counters are filled, and checks that the",
 "# публичный адрес отвечает снаружи.": "# public address answers from outside.",
 'echo "== публичный адрес =="': 'echo "== the public address =="',
 'echo "ТУННЕЛЬ НЕ ПОДНЯТ — поднимите: cloudflared tunnel --url http://127.0.0.1:8840"':
   'echo "TUNNEL IS DOWN — raise it: cloudflared tunnel --url http://127.0.0.1:8840"',
 '"снаружи: HTTP %{http_code}\\n"': '"from outside: HTTP %{http_code}\\n"',
 '"снаружи не отвечает"': '"no answer from outside"',
 'echo "== витрина локально =="': 'echo "== the showcase, locally =="',
 '"локально: HTTP %{http_code}\\n"': '"locally: HTTP %{http_code}\\n"',
 'echo "витрина не поднята — запускаю"': 'echo "the showcase is down — starting it"',
 'echo "== прогон: $SKILL на $START =="': 'echo "== run: $SKILL on $START =="',
 'echo "Готово. Открывайте на сцене: $URL"': 'echo "Done. Open on stage: $URL"',
 # demo/preflight.sh
 "# Предполётная проверка витрины. Запускать перед выходом на сцену.":
   "# Pre-flight check of the showcase. Run it before going on stage.",
 "# Проверяет каждую часть по отдельности и говорит прямо, что не так.":
   "# Checks every part on its own and says plainly what is wrong.",
 'echo "== 1. браузер, которым управляем =="': 'echo "== 1. the browser we drive =="',
 'ok "Chromium отвечает на 8830"': 'ok "Chromium answers on 8830"',
 'bad "Chromium не отвечает на 8830 — поднимите:"': 'bad "Chromium does not answer on 8830 — raise it:"',
 'echo "== 2. витрина =="': 'echo "== 2. the showcase =="',
 'ok "витрина слушает 8840"': 'ok "the showcase is listening on 8840"',
 'bad "витрина не поднята — запускаю"': 'bad "the showcase is down — starting it"',
 'ok "поднялась"': 'ok "it came up"',
 'echo "== 3. публичный адрес =="': 'echo "== 3. the public address =="',
 'bad "туннеля нет — поднимите: cloudflared tunnel --url http://127.0.0.1:8840 --no-autoupdate"':
   'bad "no tunnel — raise it: cloudflared tunnel --url http://127.0.0.1:8840 --no-autoupdate"',
 'ok "снаружи отвечает: $URL"': 'ok "answers from outside: $URL"',
 'bad "адрес $URL отдал $CODE — туннель поднялся заново, возьмите новый адрес"':
   'bad "the address $URL returned $CODE — the tunnel came up again, take the new address"',
 'echo "== 4. навыки на месте =="': 'echo "== 4. the skills are present =="',
 'print(f\\"{len(d[\'steps\'])} шаг(ов), проверен: {d.get(\'learn_declared_done\')}\\")':
   'print(f\\"{len(d[\'steps\'])} step(s), verified: {d.get(\'learn_declared_done\')}\\")',
 'bad "нет $f.skill.json"': 'bad "$f.skill.json is missing"',
 'echo "== 5. цепочка задач =="': 'echo "== 5. the task chain =="',
 "print(f'  заданий: {len(j)}' + (' — все файлы на месте' if not missing else f' — НЕТ файлов: {missing}'))":
   "print(f'  tasks: {len(j)}' + (' - every file is present' if not missing else f' - MISSING: {missing}'))",
 'echo "== 6. сухой прогон (одна задача, ~20 с) =="': 'echo "== 6. a dry run (one task, ~20 s) =="',
 'grep -q "ИТОГ ДЛЯ СЦЕНЫ"': 'grep -q "THE NUMBER FOR THE STAGE"',
 'ok "прогон прошёл"': 'ok "the run went through"',
 "print(f\\\"     на витрине: решений {len(s['steps'])} против {len(f['steps'])}, обращений к модели {s['model_calls']} против {f['model_calls']}\\\")":
   "print(f\\\"     on the showcase: {len(s['steps'])} decisions against {len(f['steps'])}, model calls {s['model_calls']} against {f['model_calls']}\\\")",
 'bad "прогон не прошёл — смотрите вывод выше"': 'bad "the run did not go through — see the output above"',
 'echo "ГОТОВО. На сцене открывать: ${URL:-<адрес не получен>}"':
   'echo "READY. Open on stage: ${URL:-<no address yet>}"',
 'echo "Страховка, если сеть откажет: demo/race.mp4"':
   'echo "The fallback if the network fails: demo/race.mp4"',
 # demo/measure.py
 "        # что отправил бы наивный агент: весь текст страницы и всё дерево доступности":
   "        # what a naive agent would send: the whole page text and the whole accessibility tree",
 'prompt = ("Состояние экрана: " + json.dumps(state, ensure_ascii=False)[:6000] +\n                  "\\nЦель: открыть список задач. Ответь ТОЛЬКО JSON {\\"idx\\": 0}.")':
   'prompt = ("Screen state: " + json.dumps(state, ensure_ascii=False)[:6000] +\n                  "\\nGoal: open the issue list. Reply with JSON ONLY {\\"idx\\": 0}.")',
 # demo/record.py
 '"""Запись прогона в mp4 — страховка на случай, если в зале не будет сети.':
   '"""Record a run to mp4 — the fallback in case the room has no network.',
 "Отдельный браузер смотрит на живую витрину и снимает её кадр за кадром, пока":
   "A separate browser watches the live showcase and shoots it frame by frame while the",
 "рантайм прогоняет демонстрацию. Из кадров собирается видео.":
   "runtime runs the demonstration. The frames are assembled into a video.",
 "# Запись идёт с этой же машины, поэтому берём локальный адрес витрины: туннель не нужен,":
   "# Recording happens on this same machine, so the local showcase address is used: no tunnel",
 "# а адрес туннеля живёт недолго и однажды уже оказался мёртвым — запись тогда сняла не то.":
   "# is needed, and a tunnel address is short-lived — one already went dead and the recording",
 "# и сняла не ту страницу.": "# then captured the wrong page.",
 "SECONDS = 78          # оставлено для совместимости":
   "SECONDS = 78          # kept for compatibility",
 "MAX_SECONDS = 180     # предохранитель: снимаем, пока идёт прогон, но не дольше":
   "MAX_SECONDS = 180     # safety stop: shoot while the run lasts, but no longer",
 "    # страница снимется этим браузером": "    # this browser is the one that shoots the page",
 "    # запускаем сам прогон": "    # start the run itself",
 'print("прогон запущен, снимаю витрину…", flush=True)': 'print("run started, shooting the showcase...", flush=True)',
 "    # Снимаем, пока идёт прогон, а не фиксированное время: запись, оборванная по таймеру,":
   "    # Shoot while the run lasts, not for a fixed time: a recording cut by a timer ends",
 "    # заканчивается кадром, где счётчики ещё не дошли до итога, и итог в кадре получается":
   "    # on a frame where the counters have not reached the verdict, so the verdict on screen",
 "    # неверным. Страховочный ролик обязан заканчиваться настоящими числами.":
   "    # is wrong. A fallback clip has to end on the real numbers.",
 "    # хвост: даём витрине показать последнюю строку итога и снимаем её":
   "    # tail: let the showcase show the final line and shoot it",
 'print(f"кадров снято: {i}", flush=True)': 'print(f"frames shot: {i}", flush=True)',
 'print(f"видео: {out} ({size//1024} КБ)")': 'print(f"video: {out} ({size//1024} KB)")',
 # probe/*
 'print("включаю трансляцию...")': 'print("turning the stream on...")',
 'print(f"кадров получено: {len(frames)} за {dt:.1f} с")': 'print(f"frames received: {len(frames)} in {dt:.1f} s")',
 'print(f"размер кадра (base64): медиана {sorted(sizes)[len(sizes)//2]} байт, макс {max(sizes)}")':
   'print(f"frame size (base64): median {sorted(sizes)[len(sizes)//2]} bytes, max {max(sizes)}")',
 'print(f"частота: {len(frames)/dt:.1f} кадр/с")': 'print(f"rate: {len(frames)/dt:.1f} frames/s")',
 "        # сохраняем последний кадр как картинку": "        # keep the last frame as an image",
 'raise SystemExit("ключа нет")': 'raise SystemExit("no key")',
 'print("всего моделей:", len(names), "| замеряю:", picks[:3])': 'print("models available:", len(names), "| measuring:", picks[:3])',
 'prompt = ("Состояние экрана: " + json.dumps(state, ensure_ascii=False) +\n          "\\nЦель: открыть обсуждение первой новости.\\nВыбери одно действие из таблицы, ответь ТОЛЬКО JSON "':
   'prompt = ("Screen state: " + json.dumps(state, ensure_ascii=False) +\n          "\\nGoal: open the discussion of the top story.\\nPick one action from the table, reply with JSON ONLY "',
 "print(f\"  {m} #{i+1}: ошибка {str(e)[:80]}\")": "print(f\"  {m} #{i+1}: error {str(e)[:80]}\")",
 "print(f\"  → медиана {sorted(ts)[len(ts)//2]:.0f} мс за одно решение\\n\")":
   "print(f\"  -> median {sorted(ts)[len(ts)//2]:.0f} ms per decision\\n\")",
 'print("до:", await ev("location.pathname"))': 'print("before:", await ev("location.pathname"))',
 "    # 1) обычный синтетический клик": "    # 1) an ordinary synthetic click",
 "return 'не найдена';": "return 'not found';",
 "a.click(); return 'нажали';": "a.click(); return 'clicked';",
 'print("клик:", r)': 'print("click:", r)',
 'print("после клика:", await ev("location.pathname"))': 'print("after the click:", await ev("location.pathname"))',
 "    # 2) полноценное событие мыши с координатами": "    # 2) a full mouse event with coordinates",
 'print("после настоящего клика мышью:", await ev("location.pathname"))':
   'print("after a real mouse click:", await ev("location.pathname"))',
 "    # (1) наивный путь: весь текст страницы, как его отдаёт любой агент":
   "    # (1) the naive path: the whole page text, the way any agent sends it",
 'print(f"НАИВНО (весь текст страницы):  {len(naive_text):>7} символов, снятие {naive_ms:6.0f} мс")':
   'print(f"NAIVE (whole page text):       {len(naive_text):>7} chars, capture {naive_ms:6.0f} ms")',
 "    # (2) структурное состояние: только интерактивные элементы с адресуемыми путями":
   "    # (2) structural state: only interactive elements, with addressable paths",
 'print(f"ПОЛНОЕ ДЕРЕВО:                 {full_size:>7} символов, снятие {ax_ms:6.0f} мс, узлов {len(nodes)}")':
   'print(f"FULL TREE:                     {full_size:>7} chars, capture {ax_ms:6.0f} ms, {len(nodes)} nodes")',
 'print(f"СТРУКТУРНОЕ СОСТОЯНИЕ:         {len(compact):>7} символов, интерактивных {len(interactive)}")':
   'print(f"STRUCTURAL STATE:              {len(compact):>7} chars, interactive {len(interactive)}")',
 'print(f"  сжатие: {full_size/max(len(compact),1):.1f}x относительно дерева, {len(naive_text)/max(len(compact),1):.1f}x относительно текста")':
   'print(f"  reduction: {full_size/max(len(compact),1):.1f}x against the tree, {len(naive_text)/max(len(compact),1):.1f}x against the text")',
 'print("  пример:", json.dumps(interactive[:3], ensure_ascii=False)[:220])':
   'print("  a sample:", json.dumps(interactive[:3], ensure_ascii=False)[:220])',
 'print("элементов:", len(data["els"]))': 'print("elements:", len(data["els"]))',
}

files = ["demo/run.sh", "demo/preflight.sh", "demo/measure.py", "demo/record.py",
         "probe/cast.py", "probe/llm_lat.py", "probe/clicktest.py", "probe/ax_probe.py",
         "probe/links.py"]
for path in files:
    text = open(path, encoding="utf-8").read()
    hits = 0
    for ru, en in M.items():
        if ru in text:
            text = text.replace(ru, en)
            hits += 1
    open(path, "w", encoding="utf-8").write(text)
    left = len(re.findall(r"[А-Яа-яЁё]", text))
    print(f"{path}: заменено {hits}, осталось символов кириллицы {left}")
