-- With no folder (opened from Finder, whose apps start in /), ask for one;
-- Cancel quits (#1102).
on run
	try
		open {choose folder with prompt "Choose a ryueda project folder"}
	end try
end run
on open theItems
	repeat with f in theItems
		do shell script "RYUEDA_APP=" & quoted form of "@BIN@/ryueda-app" & " " & quoted form of "@BIN@/ryueda" & " open " & quoted form of POSIX path of f & " > /dev/null 2>&1 &"
	end repeat
end open
