-- Stops with an error when you press x (smoke test of the crash report).
function draw(w, h)
  ui.title("Crash", "press x")
  ui.text(3, 3, "Press x to make this app fail.", "dim")
  ui.footer("x crash · l loop · q quit")
end

function key(k)
  if k == "l" then while true do end end   -- smoke test of the 2 s limit
  if k == "x" then
    local t = nil
    return t.boom
  end
end
