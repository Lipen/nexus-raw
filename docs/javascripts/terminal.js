/*
 * Replays a recorded nexus-raw session in the landing hero.
 *
 * The recording is made by `just demo-cast`: the real `nxr` binary against the
 * repo's mock server, every command and every output line stamped with the
 * time it arrived. Nothing here invents output; the player only decides when a
 * recorded line becomes visible.
 *
 * Recorded timings are the tool's own speed: most lines arrive within
 * milliseconds of each other, which no reader can follow. So each line gets a
 * minimum on-screen time and any pause longer than the maximum is shortened.
 * The raw timings stay in the cast file; only the pace is the player's.
 */
(() => {
  const MIN_GAP_MS = 130;
  const MAX_GAP_MS = 900;
  const START_DELAY_MS = 400;
  const frameSelector = ".nxr-term[data-cast]";

  const frames = [...document.querySelectorAll(frameSelector)];
  if (!frames.length) return;

  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

  for (const frame of frames) {
    const source = new URL(frame.dataset.cast, document.baseURI).href;
    load(source).then(
      (cast) => mount(frame, cast),
      () => {
        /* No cast: the still image already in the markup stays. */
      },
    );
  }

  async function load(url) {
    const response = await fetch(url, { cache: "force-cache" });
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    const cast = await response.json();
    if (!Array.isArray(cast.lines) || !cast.lines.length) throw new Error(`${url}: no lines`);
    return cast;
  }

  /** How a line is drawn: the console prefixes carry the meaning. */
  function classify(text) {
    if (text.startsWith("$ ")) return "cmd";
    if (text.startsWith("error:")) return "err";
    if (text.startsWith("hint:")) return "hint";
    if (text.startsWith("↻")) return "warn";
    if (text.startsWith("→")) return "dim";
    if (text.startsWith("○")) return "dim";
    if (/^[↑↓]/.test(text)) return "ok";
    if (/^(plan|channel|verify|uploaded|failed):/.test(text)) return "note";
    return "";
  }

  /** Reveal times: recorded timestamps, paced for a reader. */
  function schedule(lines) {
    const at = [];
    let clock = START_DELAY_MS;
    for (let i = 0; i < lines.length; i += 1) {
      if (i > 0) {
        const gap = Number(lines[i][0]) - Number(lines[i - 1][0]);
        clock += Math.min(Math.max(gap, MIN_GAP_MS), MAX_GAP_MS);
      }
      at.push(clock);
    }
    return at;
  }

  function mount(frame, cast) {
    const lines = cast.lines.filter((line) => Array.isArray(line) && typeof line[1] === "string");
    const at = schedule(lines);

    const bar = document.createElement("div");
    bar.className = "nxr-term__bar";
    const dots = document.createElement("span");
    dots.className = "nxr-term__dots";
    dots.setAttribute("aria-hidden", "true");
    const title = document.createElement("span");
    title.className = "nxr-term__title";
    title.textContent = [cast.tool, cast.scenario && `against mock-nexus (${cast.scenario})`]
      .filter(Boolean)
      .join(" · ");
    const replay = document.createElement("button");
    replay.className = "nxr-term__replay";
    replay.type = "button";
    replay.textContent = "replay";
    replay.hidden = true;
    bar.append(dots, title, replay);

    const body = document.createElement("div");
    body.className = "nxr-term__body";

    const nodes = lines.map(([, text]) => {
      const node = document.createElement("div");
      const kind = classify(text);
      node.className = kind ? `nxr-l nxr-l--${kind}` : "nxr-l";
      node.textContent = text;
      node.hidden = true;
      body.append(node);
      return node;
    });

    const cursor = document.createElement("div");
    cursor.className = "nxr-l nxr-term__cursor";
    body.append(cursor);

    frame.replaceChildren(bar, body);
    frame.dataset.ready = "true";

    if (reducedMotion.matches) {
      // No animation: the whole transcript, shown at its end.
      for (const node of nodes) node.hidden = false;
      cursor.hidden = true;
      body.scrollTop = body.scrollHeight;
      return;
    }

    let shown = 0;
    let startedAt = 0;
    let raf = 0;

    const reveal = (now) => {
      const elapsed = now - startedAt;
      let added = false;
      while (shown < nodes.length && at[shown] <= elapsed) {
        nodes[shown].hidden = false;
        shown += 1;
        added = true;
      }
      // The window keeps a fixed height: follow the tail like a real terminal.
      if (added) body.scrollTop = body.scrollHeight;
      if (shown >= nodes.length) {
        cursor.hidden = true;
        replay.hidden = false;
        raf = 0;
        return;
      }
      raf = requestAnimationFrame(reveal);
    };

    const play = () => {
      cancelAnimationFrame(raf);
      for (const node of nodes) node.hidden = true;
      cursor.hidden = false;
      replay.hidden = true;
      shown = 0;
      startedAt = performance.now();
      raf = requestAnimationFrame(reveal);
    };

    const stop = () => {
      cancelAnimationFrame(raf);
      raf = 0;
    };

    replay.addEventListener("click", () => {
      play();
      frame.scrollIntoView({ block: "nearest", behavior: "smooth" });
    });

    // Play once, when the terminal is actually on screen; a session that runs
    // off screen is a session nobody sees.
    if ("IntersectionObserver" in window) {
      const observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting && !shown && !raf) play();
          }
        },
        { threshold: 0.35 },
      );
      observer.observe(frame);
    } else {
      play();
    }
  }
})();
