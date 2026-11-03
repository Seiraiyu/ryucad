-- ryucad.app: a launcher the installer compiles (osacompile) into
-- ~/Applications. Folders dropped on its icon, or passed by
-- `open -a ryucad <folder>`, open in ryucad-app. @BIN@ is the folder the
-- installer put ryucad and ryucad-app in: an app started by Finder gets no
-- shell PATH to find them on.

on launch(args)
	do shell script quoted form of "@BIN@/ryucad-app" & args & " > /dev/null 2>&1 &"
end launch

on run
	launch("")
end run

-- A folder dropped again brings its open window forward, as `ryucad open`
-- does, rather than starting a second window on it (#411).
on open theItems
	repeat with f in theItems
		do shell script "RYUCAD_APP=" & quoted form of "@BIN@/ryucad-app" & " " & quoted form of "@BIN@/ryucad" & " open " & quoted form of POSIX path of f & " > /dev/null 2>&1 &"
	end repeat
end open
