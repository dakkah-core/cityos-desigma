"use strict";

const search = document.getElementById("search");
const grid = document.getElementById("icons");
const status = document.getElementById("status");
let host;
let icons = [];
let inserting = false;

async function readJson(path) {
  const response = await fetch(path);
  if (!response.ok) throw new Error("Cannot load plugin files. Reopen Icon Browser from Desigma.");
  return response.json();
}

async function insert(name, svg) {
  if (inserting) return;
  inserting = true;
  grid.querySelectorAll("button").forEach(button => { button.disabled = true; });
  status.textContent = `Inserting ${name}…`;
  try {
    const response = await fetch("insert-svg", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Desigma-Token": host.token },
      body: JSON.stringify({ name: `Lucide / ${name}`, svg })
    });
    const result = await response.json();
    if (!response.ok || !result.ok) throw new Error(result.error || "Insertion failed. Return to an editable document and try again.");
    status.textContent = `Inserted ${name}. Return to Desigma to edit it or Undo.`;
  } catch (error) {
    status.textContent = error instanceof TypeError
      ? "Desigma disconnected. Reopen Icon Browser from the Plugins library."
      : error.message;
  } finally {
    inserting = false;
    grid.querySelectorAll("button").forEach(button => { button.disabled = false; });
  }
}

function render() {
  grid.replaceChildren();
  const query = search.value.trim().toLowerCase();
  const matches = icons.filter(icon => icon.name.includes(query));
  for (const { name, svg, preview } of matches) {
    const button = document.createElement("button");
    button.type = "button";
    button.disabled = inserting;
    button.setAttribute("aria-label", `Insert ${name}`);
    const image = document.createElement("img");
    image.src = preview;
    image.alt = "";
    const label = document.createElement("span");
    label.textContent = name;
    button.append(image, label);
    button.addEventListener("click", () => insert(name, svg));
    grid.append(button);
  }
  if (!inserting) status.textContent = matches.length ? `${matches.length} icon${matches.length === 1 ? "" : "s"} · Select one to insert` : "No matching icons. Try another search.";
}

search.addEventListener("input", render);
window.addEventListener("pagehide", () => icons.forEach(icon => URL.revokeObjectURL(icon.preview)));
Promise.all([readJson("host.json"), readJson("icons.json")]).then(([session, catalogue]) => {
  if (session.api_version !== 1 || !session.permissions.includes("document:insert-svg")) {
    throw new Error("This session does not allow SVG insertion. Reopen the plugin from Desigma.");
  }
  host = session;
  icons = Object.entries(catalogue).map(([name, svg]) => ({
    name, svg, preview: URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }))
  }));
  search.disabled = false;
  render();
}).catch(error => { status.textContent = error.message; });
