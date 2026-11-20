-- Fetches one line from the test server and shows it, with the local time zone check (epoch 0 shows the zone offset).
local msg, err = nil, nil
app.every(600)

function tick()
  local body, status = http.get("http://10.0.2.2:18080/hello")
  if body and status == 200 then msg, err = body:gsub("%s+$", ""), nil else msg, err = nil, status or body end
end

function draw(w, h)
  ui.title("Hello", "net test")
  ui.text(3, 3, msg or "Loading…", "accent")
  if err then ui.text(3, 4, "Offline: " .. tostring(err), "bad") end
  ui.text(3, 5, "tz " .. os.date("%H:%M", 0), "dim")
  ui.footer("q quit")
end
