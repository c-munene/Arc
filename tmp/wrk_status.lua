local threads = {}

function setup(thread)
  table.insert(threads, thread)
end

function init(args)
  c200 = 0
  c400 = 0
  c404 = 0
  c429 = 0
  c502 = 0
  c503 = 0
  c504 = 0
  c_other = 0
end

function response(status, headers, body)
  if status == 200 then c200 = c200 + 1
  elseif status == 400 then c400 = c400 + 1
  elseif status == 404 then c404 = c404 + 1
  elseif status == 429 then c429 = c429 + 1
  elseif status == 502 then c502 = c502 + 1
  elseif status == 503 then c503 = c503 + 1
  elseif status == 504 then c504 = c504 + 1
  else c_other = c_other + 1
  end
end

function done(summary, latency, requests)
  local t200, t400, t404, t429, t502, t503, t504, tother = 0,0,0,0,0,0,0,0
  for _, thread in ipairs(threads) do
    t200 = t200 + (thread:get("c200") or 0)
    t400 = t400 + (thread:get("c400") or 0)
    t404 = t404 + (thread:get("c404") or 0)
    t429 = t429 + (thread:get("c429") or 0)
    t502 = t502 + (thread:get("c502") or 0)
    t503 = t503 + (thread:get("c503") or 0)
    t504 = t504 + (thread:get("c504") or 0)
    tother = tother + (thread:get("c_other") or 0)
  end
  io.write(string.format("\nSTATUS_COUNTS 200=%d 400=%d 404=%d 429=%d 502=%d 503=%d 504=%d other=%d\n", t200, t400, t404, t429, t502, t503, t504, tother))
end
