A = 0
B = 0
OTHER = 0
OK = 0
ERR = 0

response = function(status, headers, body)
  if status == 200 then
    OK = OK + 1
  else
    ERR = ERR + 1
  end

  if body == "A\n" then
    A = A + 1
  elseif body == "B\n" then
    B = B + 1
  else
    OTHER = OTHER + 1
  end
end

done = function(summary, latency, requests)
  io.write(string.format("\nLUA_COUNTS A=%d B=%d OTHER=%d OK=%d ERR=%d\n", A, B, OTHER, OK, ERR))
end
