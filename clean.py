import subprocess

print("cleaning...")
subprocess.run(["cargo", "fix", "--all-targets", "--allow-dirty"])
subprocess.run(["cargo", "fmt"])
print("cleaned.")
