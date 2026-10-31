#!/usr/bin/env python3
"""Перевод всего интерфейса рантайма на английский. Один проход по словарю.

Словарь применяется к исходникам, поэтому строки должны совпадать дословно;
после замены скрипт печатает всё, что осталось кириллицей, — чтобы ничего не
осталось незамеченным.
"""
import re
import sys

M = {
    # ── вывод рантайма: прогон цепочкой ────────────────────────────────────
    "  → {:.1} с, {slow_steps} решений\\n": "  → {:.1} s, {slow_steps} decisions\\n",
    "  → {:.1} с, {} вызовов модели, {} шагов → {}": "  → {:.1} s, {} model calls, {} steps → {}",
    "  → {:.1} с, итог: {slow_verdict} | модель {slow_opinion}": "  → {:.1} s, result: {slow_verdict} | the model says: {slow_opinion}",
    "  {:>2}/{total} {:<22} {} решений {}, модель {}, поправок {}, {:.2} с": "  {:>2}/{total} {:<22} {} decisions {}, model {}, repairs {}, {:.2} s",
    "  → {:.2} с, {escalated} обращений к модели, итог: {fast_verdict}": "  → {:.2} s, {escalated} model calls, result: {fast_verdict}",
    "  → {:.2} с, {fast_steps} решений, обращений к модели {escalated}\\n": "  → {:.2} s, {fast_steps} decisions, model calls {escalated}\\n",
    "  → {:.2} с, {} обращений к модели, {} шагов → {}": "  → {:.2} s, {} model calls, {} steps → {}",
    "{fast_steps} решений, обращений к модели {escalated}, поправок {corrections}": "{fast_steps} decisions, model calls {escalated}, repairs {corrections}",
    "  mint:         {:.2} с, {done} шагов, обращений к модели {escalated}": "  mint:         {:.2} s, {done} steps, model calls {escalated}",
    "  mint:       {:.2} с, {} обращений, {} самопочинок → {fast_url}": "  mint:       {:.2} s, {} model calls, {} self-repairs → {fast_url}",
    "  mint:         {:>7.2} с, {fast_steps} решений, модель вызывалась {escalated} раз": "  mint:         {:>7.2} s, {fast_steps} decisions, the model was asked {escalated} times",
    "MINT — из памяти, на тех же страницах с другими данными": "MINT — from memory, on the same pages with different data",
    "\\nMINT — исполняется запомненная программа:": "\\nMINT — running the remembered program:",
    "\\n  выполнено {ok} из {total}; без обращения к модели {no_model} из {total}": "\\n  completed {ok} of {total}; with no model call at all: {no_model} of {total}",
    "\\nИТОГ ДЛЯ СЦЕНЫ": "\\nTHE NUMBER FOR THE STAGE",
    "\\nИТОГ": "\\nSUMMARY",
    "\\nитог: {} шагов, {} вызовов модели, {:.1} с": "\\nresult: {} steps, {} model calls, {:.1} s",
    "\\nОБЫЧНЫЙ АГЕНТ": "\\nORDINARY AGENT",
    "\\nпервые элементы:\\n{}": "\\nfirst elements:\\n{}",
    "{slow_steps} решений, каждое — обращение к модели": "{slow_steps} decisions, every one of them a model call",
    "{total} задач репозиториев подряд: последний коммит и список задач на разных проектах": "{total} repository tasks in a row: the latest commit and the issue list across different projects",
    "  обычный агент: {:>7.1} с, {slow_steps} решений": "  ordinary agent: {:>7.1} s, {slow_steps} decisions",
    "  обычный агент: {:.1} с, {n} шагов, каждый — модель": "  ordinary agent: {:.1} s, {n} steps, every one a model call",
    "  на одно решение: {:.0} мс против {:.3} мс — в {:.0} раз быстрее": "  per decision: {:.0} ms against {:.3} ms — {:.0}x faster",
    "  вся цепочка: {:.1} с против {:.2} с — в {:.1} раз": "  whole chain: {:.1} s against {:.2} s — {:.1}x",
    "  решений на ту же работу: ": "  decisions for the same work: ",
    "витрина: {relay} | задач: {total}\\n": "showcase: {relay} | tasks: {total}\\n",
    "витрина: {relay} | цель: {goal}": "showcase: {relay} | goal: {goal}",
    "  задача {}/{total}: решение не получено ({e})": "  task {}/{total}: no decision came back ({e})",
    "  задача {}/{total}, шаг {fast_steps}: {} «{}» — {:.2} мс ({note})": "  task {}/{total}, step {fast_steps}: {} «{}» — {:.2} ms ({note})",
    "  задача {}/{total}, шаг {slow_steps}: {} «{}» — {:.0} мс модели": "  task {}/{total}, step {slow_steps}: {} «{}» — {:.0} ms of model",
    "  модель недоступна ({e}) — задача пропущена, цепочка продолжается": "  the model is unavailable ({e}) — task skipped, the chain carries on",
    "  модель недоступна ({e}) — задача пропущена": "  the model is unavailable ({e}) — task skipped",
    "  модель тоже не нашла хода — останавливаюсь": "  the model found no move either — stopping",
    "   модель: цель достигнута ({})": "   model: goal reached ({})",
    "  модель: цель достигнута": "  model: goal reached",
    "  убрано лишних вкладок перед прогоном: {closed}": "  leftover tabs closed before the run: {closed}",
    "  убрано лишних вкладок: {closed}\\n": "  leftover tabs closed: {closed}\\n",
    "  убрано лишних вкладок: {closed}": "  leftover tabs closed: {closed}",
    # ── вывод рантайма: обучение и одиночный прогон ────────────────────────
    "ПЕРВЫЙ ПРОГОН — модель решает каждый шаг\\nцель: {goal}\\nстраница: {start}\\n": "FIRST RUN — the model decides every step\\ngoal: {goal}\\npage: {start}\\n",
    "НОВЫЕ ДАННЫЕ, ТОТ ЖЕ НАВЫК\\n": "NEW DATA, THE SAME SKILL\\n",
    "  шаг {}: {} «{}» — {} ({:.1} мс модели)": "  step {}: {} «{}» — {} ({:.1} ms of model)",
    "  пустой ход: нажатие «{}» ничего не изменило — говорю модели и продолжаю": "  empty move: clicking «{}» changed nothing — telling the model and carrying on",
    "  указатель был устаревшим, но страница сменилась на «{now_page}» — шаг принят": "  the handle was stale but the page moved to «{now_page}» — step accepted",
    "  элемент исчез до нажатия — смотрю ещё раз": "  the element vanished before the click — looking again",
    "  шаг не выполнен и повторить не удалось — останавливаюсь": "  the step did not happen and could not be retried — stopping",
    "  разворот: страница «{}» уже была — шаг отброшен, прогон обрезан здесь": "  reversal: the page «{}» was already visited — step dropped, the run is cut here",
    "  шаг {} сделал не то, что обещал (ждали «{}», получили «{}») — исправляю": "  step {} did not do what it promised (expected «{}», got «{}») — repairing",
    "  шаг {}: элемент переехал, но нашёлся заново по {}": "  step {}: the element moved, but was found again by {}",
    "  шаг {}: элемент исчез и заново не нашёлся": "  step {}: the element vanished and was not found again",
    "  шаг {}: не узнан — спрашиваю модель": "  step {}: not recognised — asking the model",
    "  шаг {} переучен на {} «{}»": "  step {} re-taught to {} «{}»",
    "проверка по цели: НЕ достигнута — {}": "goal check: NOT reached — {}",
    "проверка по цели: достигнута — {}": "goal check: reached — {}",
    "\\nитог: {} шагов, {} вызовов модели, {:.1} с": "\\nresult: {} steps, {} model calls, {:.1} s",
    "ВНИМАНИЕ: навык помечен как непроверенный — не выдавать его успех за доказанный": "WARNING: this skill is marked unverified — do not present its success as proven",
    "опыт записан как программа: {out}": "the experience is written down as a program: {out}",
    "пришли на: ": "landed on: ",
    "на чём учились: {}": "learned on: {}",
    "во что это обошлось тогда: {} вызовов модели, {:.1} с": "what it cost back then: {} model calls, {:.1} s",
    "обучение подтвердило успех: {}\\n": "the goal check confirmed success: {}\\n",
    "считает, что достигнута": "says it was reached",
    "считает, что не достигнута": "says it was not reached",
    "  цель достигнута: {}": "  goal reached: {}",
    "  шаг {}: {} «{}» — {:.2} мс решения ({note})": "  step {}: {} «{}» — {:.2} ms of decision ({note})",
    "  шаг {}: {} «{}» — узнан по {} ({:.2} мс решения)": "  step {}: {} «{}» — recognised by {} ({:.2} ms of decision)",
    "  шаг {n}: {} «{}» — {:.0} мс модели": "  step {n}: {} «{}» — {:.0} ms of model",
    "  левый путь: ": "  the slow path: ",
    # ── вывод зондов и замера ─────────────────────────────────────────────
    "страница: {here}": "page: {here}",
    "адресуемых элементов: {}": "addressable elements: {}",
    "переход: {:.0} мс | снятие состояния: {:.1} мс": "navigation: {:.0} ms | state capture: {:.1} ms",
    "наш пакет состояния: {payload} байт": "our state payload: {payload} bytes",
    "наивный путь (текст страницы или всё дерево доступности): {naive_bytes} байт, {naive_nodes} узлов": "naive path (page text or the whole accessibility tree): {naive_bytes} bytes, {naive_nodes} nodes",
    "разница: {:.0}x меньше": "difference: {:.0}x smaller",
    "  отчёт: {out_path}": "  report: {out_path}",
    "оценка: {total} задач\\n": "evaluation: {total} tasks\\n",
    "ВЫПОЛНЕНО": "DONE    ",
    "ПРОВАЛ   ": "FAILED  ",
    "  {:>2}/{total} {:<22} {} решений {}, модель {}, поправок {}, {:.2} с": "  {:>2}/{total} {:<22} {} decisions {}, model {}, repairs {}, {:.2} s",
    "браузер": "browser",
    "задачи проекта": "project tasks",
    "исходники проекта": "project sources",
    # ── категории и найденные тиры ────────────────────────────────────────
    '"форме"': '"shape"',
    '"словам"': '"words"',
    '"подсказке"': '"the hint"',
    "узнан по {}": "recognised by {}",
    "не узнан — спрашивал модель": "not recognised — asked the model",
    "«{}» уже была — шаг отброшен": "«{}» was already visited — step dropped",
}

# relay.py: страница витрины
R = {
    "Ждём запуска…": "Waiting for the run…",
    "Две панели. Смотрите на счётчики решений.": "Two panes. Watch the decision counters.",
    "Цель: ": "Goal: ",
    "завершено": "finished",
    "работает…": "working…",
    "наблюдаем за браузером…": "watching the browser…",
    "пока ни одного шага": "no steps yet",
    "решения, сумма": "decisions, total",
    "обращений к модели": "model calls",
    "вся задача": "whole task",
    "'модель '+ms(st.decision_us)+' мс'": "'model '+ms(st.decision_us)+' ms'",
    "'память '+ms(st.decision_us)+' мс'": "'memory '+ms(st.decision_us)+' ms'",
    "<small>мс</small>": "<small>ms</small>",
    "<small>с</small>": "<small>s</small>",
    "м[1]+' задач подряд. Две панели. Смотрите на счётчики решений.'": "m[1]+' tasks in a row. Two panes. Watch the decision counters.'",
    "'ru-RU'": "'en-US'",
    "Итог.</b> Решений принято: ${slow.steps.length} у обычного агента, ${fast.steps.length} у mint `": "Result.</b> Decisions made: ${slow.steps.length} by the ordinary agent, ${fast.steps.length} by mint `",
    "(и это на ту же работу). ": "(and that is for the same work). ",
    "На одно решение: <b>${fmtUs(perSlow)} мкс</b> против <b>${fmtUs(perFast)} мкс</b> — в <b>${Math.round(ratio).toLocaleString('en-US')} раз</b> быстрее. ": "Per decision: <b>${fmtUs(perSlow)} µs</b> against <b>${fmtUs(perFast)} µs</b> — <b>${Math.round(ratio).toLocaleString('en-US')}x</b> faster. ",
    "Обращений к модели: <b>${slow.model_calls}</b> против <b>${fast.model_calls}</b>. ": "Model calls: <b>${slow.model_calls}</b> against <b>${fast.model_calls}</b>. ",
    "Проверка (не мнение):": "Verdict (not an opinion):",
    "Мнение модели отдельно, как мнение:": "The model's opinion, kept separate as an opinion:",
    "витрина слушает 0.0.0.0:{PORT}": "showcase listening on 0.0.0.0:{PORT}",
    "  тишина в логах": "  quiet logs",
    "Обычный агент": "Ordinary agent",
    "исполняется запомненный навык": "running the remembered skill",
    "модель решает каждый шаг": "the model decides every step",
}

passed = failures = 0
for path, mapping in (("src/main.rs", M), ("demo/relay.py", R), ("src/skill.rs", M)):
    try:
        text = open(path, encoding="utf-8").read()
    except FileNotFoundError:
        print(f"нет файла {path}")
        continue
    for ru, en in mapping.items():
        if ru in text:
            text = text.replace(ru, en)
            passed += 1
    open(path, "w", encoding="utf-8").write(text)

print(f"заменено: {passed}")
print("\n=== что осталось кириллицей ===")
for path in ("src/main.rs", "src/skill.rs", "src/decide.rs", "src/cdp.rs", "src/state.rs",
             "demo/relay.py", "src/help.txt"):
    try:
        for i, line in enumerate(open(path, encoding="utf-8"), 1):
            if re.search(r"[А-Яа-яЁё]", line) and not line.strip().startswith("//") \
               and not line.strip().startswith("///") and not line.strip().startswith("#"):
                print(f"{path}:{i}: {line.strip()[:110]}")
    except FileNotFoundError:
        pass
