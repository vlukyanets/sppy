--[[
sppy control interface for VLC 3 (https://github.com/vlukyanets/sppy).

sppy copies this file into VLC's Lua interface folder and starts VLC with
  --extraintf=luaintf --lua-intf=sppy --lua-config=sppy={port='…',secret='…'}
The module connects to sppy on 127.0.0.1, says hello with the secret and then answers
one line per command. VLC's Lua cannot wait with a timeout, so it never speaks first:
sppy asks for the status many times a second.

Every number on the wire is an integer (time in microseconds, rate in millionths):
VLC applies the system locale, under which Lua reads and prints fractions with a comma.
]]

local VERSION = "2"

local function int(x)
  return math.floor((x or 0) + 0.5)
end

local function fields(line)
  local out = {}
  local start = 1
  while true do
    local tab = string.find(line, "\t", start, true)
    if not tab then
      table.insert(out, string.sub(line, start))
      return out
    end
    table.insert(out, string.sub(line, start, tab - 1))
    start = tab + 1
  end
end

local function status()
  local input = vlc.object.input()
  if not input then
    return "status\tnone"
  end
  local item = vlc.input.item()
  local title = item and item:name() or ""
  title = string.gsub(title, "%c", " ")
  return "status\t" .. string.format("%d\t%d\t%d\t%d\t%d\t",
    int(vlc.var.get(input, "time")),
    int(vlc.var.get(input, "length")),
    int(vlc.var.get(input, "state")),
    int(vlc.var.get(input, "rate") * 1000000),
    int(vlc.var.get(input, "cache") * 1000)) .. title
end

local osd_channel = nil

local function run(f)
  local cmd = f[1]
  local input = vlc.object.input()
  if cmd == "status" then
    return status()
  elseif cmd == "load" then
    -- load <uri> <audio languages> <subtitle languages> <title> [<name=value> ...]
    local options = { "start-paused" }
    if f[3] and f[3] ~= "" then table.insert(options, "audio-language=" .. f[3]) end
    if f[4] and f[4] ~= "" then table.insert(options, "sub-language=" .. f[4]) end
    for i = 6, #f do table.insert(options, f[i]) end
    local item = { path = f[2], options = options }
    if f[5] and f[5] ~= "" then item.name = f[5] end
    vlc.playlist.clear()
    vlc.playlist.add({ item })
  elseif cmd == "sub" then
    if not input then return "error\tnothing is open" end
    vlc.input.add_subtitle_mrl(f[2])
  elseif cmd == "seek" then
    if not input then return "error\tnothing is open" end
    vlc.var.set(input, "time", tonumber(f[2]))
  elseif cmd == "play" then
    if vlc.playlist.status() == "paused" then vlc.playlist.pause() end
  elseif cmd == "pause" then
    if vlc.playlist.status() == "playing" then vlc.playlist.pause() end
  elseif cmd == "rate" then
    if not input then return "error\tnothing is open" end
    vlc.var.set(input, "rate", tonumber(f[2]) / 1000000)
  elseif cmd == "osd" then
    -- Without a video window (nothing open, audio only) there is nowhere to show it.
    if not osd_channel then
      local ok, channel = pcall(vlc.osd.channel_register)
      if ok then osd_channel = channel end
    end
    if osd_channel then
      pcall(vlc.osd.message, f[3] or "", osd_channel, "top-left", tonumber(f[2]))
    end
  elseif cmd == "clear" then
    vlc.playlist.clear()
  elseif cmd == "quit" then
    vlc.misc.quit()
  else
    return "error\tunknown command " .. tostring(cmd)
  end
  return "ok"
end

local fd = vlc.net.connect_tcp("127.0.0.1", tonumber(config.port))
if not fd or fd < 0 then
  vlc.msg.err("sppy: cannot connect to sppy on port " .. tostring(config.port))
  vlc.misc.quit()
  return
end
vlc.net.send(fd, "hello\t" .. tostring(config.secret) .. "\t" .. VERSION .. "\n")

local buffer = ""
while true do
  vlc.net.poll({ [fd] = vlc.net.POLLIN })
  local data = vlc.net.recv(fd, 4096)
  if not data or data == "" then break end
  buffer = buffer .. data
  while true do
    local nl = string.find(buffer, "\n", 1, true)
    if not nl then break end
    local line = string.gsub(string.sub(buffer, 1, nl - 1), "\r$", "")
    buffer = string.sub(buffer, nl + 1)
    local ok, reply = pcall(run, fields(line))
    if not ok then reply = "error\t" .. string.gsub(tostring(reply), "%c", " ") end
    vlc.net.send(fd, reply .. "\n")
  end
end

-- sppy is gone: a player nobody controls would only drift away from the room.
vlc.net.close(fd)
vlc.misc.quit()
