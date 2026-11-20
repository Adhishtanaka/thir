local items = store.get("items", {})
if type(items) ~= "table" then items = {} end
local sel = 1

local function save() store.set("items", items) end

-- pieces in their own colours, one after the other on one row
local function parts(x, y, list)
  for _, p in ipairs(list) do ui.text(x, y, p[1], p[2]); x = x + ui.width(p[1]) end
end

function draw(w, h)
  local cw = math.min(w - 3, 60)
  local done = 0
  for _, it in ipairs(items) do if it.done then done = done + 1 end end
  ui.title("To-do", #items .. " tasks")
  ui.text(2, 3, "TODO", "accent")
  parts(2, 4, {{tostring(#items), "cyan"}, {" tasks", "text"}})
  ui.text(2, 6, ui.pad("#  S  TASK", cw), "accent")
  ui.text(2, 7, string.rep("─", cw))
  if #items == 0 then ui.text(2, 8, "No tasks yet. Press a to add one.", "cyan") end
  for i, it in ipairs(items) do
    local y = 7 + i
    if y < h - 3 then
      if i == sel then ui.fill(2, y, cw, 1, "sel") end
      local c = i == sel and "sel" or (it.done and "good" or "text")
      ui.text(2, y, string.format("%-2d", i), i == sel and "sel" or "text")
      ui.text(5, y, it.done and "✓" or "○", c)
      ui.text(8, y, ui.pad(tostring(it.text), cw - 7), i == sel and "sel" or "text")
    end
  end
  local y = math.min(8 + math.max(#items, 1), h - 3)
  ui.text(2, y, string.rep("─", cw))
  parts(2, y + 1, {{#items .. " total", "text"}, {" · ", "dim"}, {done .. " done", "good"}, {" · ", "dim"}, {(#items - done) .. " open", "cyan"}})
  ui.footer("↑↓ move · space tick · a add · d delete · q quit")
end

function key(k)
  if k == "up" then sel = math.max(1, sel - 1)
  elseif k == "down" then sel = math.min(math.max(#items, 1), sel + 1)
  elseif k == " " and items[sel] then items[sel].done = not items[sel].done; save()
  elseif k == "a" then
    local t = ui.input("New task: ")
    if t and t ~= "" then table.insert(items, {text = t, done = false}); sel = #items; save() end
  elseif k == "d" and items[sel] and ui.confirm("Delete this task?") then
    table.remove(items, sel); sel = math.max(1, math.min(sel, #items)); save()
  end
end
