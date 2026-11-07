// Fixture app: Howler only. One sprite map is a literal in this bundle; two
// more are JSON files the app fetches (Howler shape and audiosprite shape).
import { Howl } from "howler";

const sfx = new Howl({
  src: ["assets/audio/sfx.webm", "assets/audio/sfx.mp3"],
  sprite: {
    spin: [0, 500],
    stop: [1500, 750],
    bonus_loop: [3000, 1000, true],
  },
});

async function loadJsonSprite(url: string) {
  const map = await (await fetch(url)).json();
  return new Howl({ src: map.src, sprite: map.sprite });
}

type JukeboxPart = { start: number; end: number; loop: boolean };

async function loadJukebox(url: string) {
  const map = await (await fetch(url)).json();
  const sprite: Record<string, [number, number, boolean]> = {};
  for (const [name, part] of Object.entries<JukeboxPart>(map.spritemap)) {
    sprite[name] = [part.start * 1000, (part.end - part.start) * 1000, part.loop];
  }
  return new Howl({ src: map.resources.map((file: string) => "assets/audio/" + file), sprite });
}

document.addEventListener(
  "pointerdown",
  async () => {
    sfx.play("spin");
    (await loadJsonSprite("assets/audio/music.json")).play("base");
    (await loadJukebox("assets/audio/ui.json")).play("tap");
  },
  { once: true },
);
