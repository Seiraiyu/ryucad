on launch(args)
	do shell script quoted form of "@BIN@/ryueda-app" & args & " > /dev/null 2>&1 &"
end launch
on run
	launch("")
end run
on open theItems
	repeat with f in theItems
		do shell script "RYUEDA_APP=" & quoted form of "@BIN@/ryueda-app" & " " & quoted form of "@BIN@/ryueda" & " open " & quoted form of POSIX path of f & " > /dev/null 2>&1 &"
	end repeat
end open
