# Mino v1.1.0 — Terminal Demo
# Record with: vhs demo.tape
# Output: demo.gif

Output demo.gif
Set FontSize 18
Set Width 1200
Set Height 650
Set Padding 24
Set Theme "Catppuccin Mocha"
Set WindowBar "Colorful"
Set TypingSpeed 40ms
Set Shell "zsh"
Set Framerate 30

# Step 1: Launch sandbox — triggers interactive layer picker
Type "mino run"
Sleep 500ms
Enter
Sleep 2s

# Step 2: Interactive layer selection — typescript is first item
Space
Sleep 500ms
Enter
Sleep 1s

# Step 3: Save prompt — navigate to "Don't save" (third option)
Down 2
Sleep 500ms
Enter
Sleep 4s

# Step 4: Inside container
Type "echo 'Hello from Mino!'"
Sleep 300ms
Enter
Sleep 1.5s

Type "whoami"
Sleep 300ms
Enter
Sleep 1.5s

Type "node --version"
Sleep 300ms
Enter
Sleep 1.5s

# Step 5: Clean exit
Type "exit"
Sleep 300ms
Enter
Sleep 1.5s
