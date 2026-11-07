// Fixture app: Pixi v8, antialias on, resolution uncapped, WebGL preferred.
// The generator inlines the minified bundle into index.html, the way a
// SvelteKit inline build does. All hosts use the reserved .invalid TLD.
import { Application, Assets, Sprite } from "pixi.js";
import { Howl } from "howler";

const query = new URLSearchParams(location.search);
const rgsHost = query.get("rgs_url") || "";
const sessionID = query.get("sessionID") || "";

async function callRgs(path: string, body: Record<string, unknown>) {
  const response = await fetch(`https://${rgsHost}${path}`, {
    method: "POST",
    body: JSON.stringify({ sessionID, ...body }),
  });
  return response.json();
}

async function start() {
  const app = new Application();
  await app.init({
    antialias: true,
    resolution: window.devicePixelRatio,
    autoDensity: true,
    preference: "webgl",
    background: "#0b0e17",
    resizeTo: window,
  });
  document.body.appendChild(app.canvas);

  const reels = await Assets.load("assets/images/reels-2048x2048.png");
  app.stage.addChild(new Sprite(reels));

  const soundList = await (await fetch("assets/audio/sounds.json")).json();
  const music = new Howl({ src: soundList.src, sprite: soundList.sprite });
  music.play("base_loop");

  await callRgs("/wallet/authenticate", { language: "en" });
  document.addEventListener("keydown", async (event) => {
    if (event.code !== "Space") return;
    await callRgs("/wallet/play", { amount: 1000000, mode: "BASE" });
    await callRgs("/bet/event", { event: "1" });
    await callRgs("/wallet/end-round", {});
  });

  const feed = new WebSocket("wss://live.example.invalid/feed");
  feed.addEventListener("message", (message) => console.log(message.data));
  fetch("https://api.example.invalid/v1/config?session=" + sessionID).then((r) => r.json());
  const logo = new Image();
  logo.src = "https://cdn.example.invalid/brand/logo.png?v=3";
}

start();
