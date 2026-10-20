# Sphere's shell integration for bash. Sphere starts bash as `bash --rcfile <this file> -i`.
#
# It tells Sphere's terminal when a command starts and ends, with its exit status and the directory it ran in, so a
# failed command can offer "Fix it". Sphere reads these sequences from the terminal's output:
#   OSC 7700 ; cmd ; <base64 of the command line> ; <nonce> BEL    the command about to run
#   OSC 133 ; C ; sphere=<nonce> BEL                                 its output starts
#   OSC 7 ; file://host/path BEL                                     the working directory
#   OSC 133 ; D ; <exit status> ; sphere=<nonce> BEL                 the command finished
#   OSC 133 ; A ; sphere=<nonce> BEL                                 the prompt is about to be drawn
#
# The nonce is a secret that Sphere gives this one shell in SPHERE_SHELL_NONCE, and Sphere believes only the marks
# that carry it, so nothing else that prints to the terminal can pose as this shell. It is copied to a variable
# and unset first of all, before your files run, so nothing you start inherits it. (VS Code does the same with
# VSCODE_NONCE.) Without one (bash started by hand with this file) no hooks are added.
#
# Adapted from the shell integration scripts of VS Code (https://github.com/microsoft/vscode, MIT License,
# Copyright (c) Microsoft Corporation): the login emulation, the DEBUG trap and the PROMPT_COMMAND handling follow
# its shellIntegration-bash.sh.
#
# This file must never break your shell: the hooks swallow their own errors, and SPHERE_SHELL_INTEGRATION=0 turns
# it off. It works with the bash 3.2 that macOS ships, so it keeps to what that version understands. (The DEBUG
# trap does not run for a command line that begins with a subshell, `(cd sub && make)`: that one is not reported.)

# A login shell ignores --rcfile, so this file stands in for the login files: /etc/profile, then the first of
# ~/.bash_profile, ~/.bash_login and ~/.profile (what `bash -l` reads).
if [ -z "${__sphere_loaded-}" ]; then
	__sphere_loaded=1
	__sphere_nonce=${SPHERE_SHELL_NONCE-}
	builtin unset SPHERE_SHELL_NONCE
	if [ -r /etc/profile ]; then
		. /etc/profile
	fi
	if [ -r ~/.bash_profile ]; then
		. ~/.bash_profile
	elif [ -r ~/.bash_login ]; then
		. ~/.bash_login
	elif [ -r ~/.profile ]; then
		. ~/.profile
	fi

	# PWD as a file URL path. Percent-encoded byte by byte, so that nothing in a directory name (a space, a
	# newline, an escape) can end or forge a sequence. Done only when the directory changes.
	__sphere_encode_pwd() {
		builtin local LC_ALL=C
		builtin local s="$PWD" out= c i n
		for ((i = 0; i < ${#s}; i++)); do
			c=${s:i:1}
			case $c in
				[-/._~0-9A-Za-z]) out=$out$c ;;
				*)
					builtin printf -v n '%d' "'$c"
					builtin printf -v c '%%%02X' $((n & 255))
					out=$out$c
					;;
			esac
		done
		__sphere_url=$out
	}

	# First in the PROMPT_COMMAND: the directory, the end of the command that just ran, the start of the prompt.
	__sphere_pc_begin() {
		__sphere_status=$?
		{
			builtin local d=
			if [ "$PWD" != "${__sphere_pwd-}" ]; then
				__sphere_pwd=$PWD
				__sphere_encode_pwd
			fi
			if [ "${__sphere_ran-}" = 1 ]; then
				__sphere_ran=0
				d=$'\033]133;D;'$__sphere_status$';sphere='$__sphere_nonce$'\a'
			fi
			builtin printf '\033]7;file://%s%s\a%s\033]133;A;sphere=%s\a' "${HOSTNAME-}" "${__sphere_url-}" "$d" "$__sphere_nonce"
		} 2>/dev/null || :
		# your own PROMPT_COMMAND sees the status the command left
		builtin return "$__sphere_status"
	}

	# The last entry of the history: its number in __sphere_hn and its line in __sphere_hl.
	__sphere_hist() {
		__sphere_hl=$(HISTTIMEFORMAT= builtin history 1 2>/dev/null)
		__sphere_hn=${__sphere_hl#"${__sphere_hl%%[![:space:]]*}"}
		__sphere_hn=${__sphere_hn%%[!0-9]*}
		__sphere_hl=${__sphere_hl#*[[:digit:]][* ] }
	}

	# Last in the PROMPT_COMMAND: from here the next command to start is one you typed. The flag is set by the
	# arithmetic in the very last statement, because with extdebug or functrace the trap also runs inside
	# functions, and a statement after the flag would look like your command. The arithmetic gives back the
	# status you had, so your next command sees it.
	__sphere_pc_end() {
		# the first prompt: the history entry the next line has to be newer than, if history keeps it
		if [ -z "${__sphere_histn+x}" ]; then
			__sphere_hist
			__sphere_histn=$__sphere_hn
		fi
		builtin return $((__sphere_ready = 1, ${__sphere_status-0}))
	}

	# A command line starts: tell Sphere what it is. $1 is the line.
	__sphere_started() {
		builtin local line="${1-}" b64
		__sphere_ran=1
		b64=$(\command base64 <<<"${line:0:4096}" 2>/dev/null)
		builtin printf '\033]7700;cmd;%s;%s\a\033]133;C;sphere=%s\a' "${b64//$'\n'/}" "$__sphere_nonce" "$__sphere_nonce"
	}

	# With bash-preexec loaded (iTerm2's integration, atuin...): it tells your commands from the prompt's already
	# and passes the line as $1, so use its hook instead of a second DEBUG trap.
	__sphere_preexec() {
		{
			__sphere_started "${1-}"
		} 2>/dev/null || :
		builtin return 0
	}

	# The DEBUG trap runs before every simple command, but only the first one after the prompt starts a command
	# line: not the prompt's own commands (the first of them is ours, which also clears the flag when the last
	# prompt got no command, as after an empty line), a key binding (bind -x) or a completion.
	__sphere_debug() {
		case ${BASH_COMMAND-} in
			__sphere_pc_begin*)
				__sphere_ready=
				builtin return 0
				;;
		esac
		{
			[ -n "${__sphere_ready-}" ] && [ -z "${READLINE_POINT-}" ] && [ -z "${COMP_POINT-}" ] || builtin return 0
			builtin local cmd=
			__sphere_ready=
			# The line as you typed it. BASH_COMMAND has aliases expanded and is only the first command of a list,
			# so history is better. A line that history left out (HISTCONTROL=ignorespace or ignoredups,
			# HISTIGNORE...) leaves the last entry the same as it was for the line before, and that is an older
			# line: it is still this one when it begins with this command, as a repeated line does. (Bash 3.2 says
			# history is off inside its own traps, so there it is trusted to be on.)
			if ((BASH_VERSINFO[0] < 4)) || [[ -o history ]]; then
				__sphere_hist
				cmd=$__sphere_hl
				if [ "$__sphere_hn" = "${__sphere_histn-}" ]; then
					case $cmd in
						"$BASH_COMMAND"*) ;;
						*) cmd= ;;
					esac
				fi
				__sphere_histn=$__sphere_hn
			fi
			if [ -z "$cmd" ]; then
				cmd=$BASH_COMMAND
			fi
			__sphere_started "$cmd"
		} 2>/dev/null || :
		builtin return 0
	}

	# Everything but the DEBUG trap itself, which is set below, outside any function. $1 is the DEBUG trap that is
	# there already ('' when there is none); the trap to set is left in __sphere_trap ('' when bash-preexec has
	# the DEBUG trap).
	__sphere_install() {
		__sphere_ran=0
		__sphere_ready=
		__sphere_pwd=
		__sphere_url=
		__sphere_trap=
		if [ -n "${bash_preexec_imported-}${__bp_imported-}" ]; then
			precmd_functions=(__sphere_pc_begin ${precmd_functions[@]+"${precmd_functions[@]}"})
			preexec_functions=(${preexec_functions[@]+"${preexec_functions[@]}"} __sphere_preexec)
		else
			if [ -z "${1-}" ]; then
				__sphere_trap='__sphere_debug "$_"'
			else
				# keep a trap that is there already: ours runs first, then it
				__sphere_trap='__sphere_debug "$_"'$'\n'"$1"
			fi
			if ((BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1))); then
				# bash 5.1 runs every element of a PROMPT_COMMAND array
				PROMPT_COMMAND=(__sphere_pc_begin "${PROMPT_COMMAND[@]}" __sphere_pc_end)
			else
				PROMPT_COMMAND=__sphere_pc_begin${PROMPT_COMMAND:+$'\n'$PROMPT_COMMAND}$'\n'__sphere_pc_end
			fi
		fi
		__sphere_hooked=1
	}

	# Hooks go in for an interactive shell on a terminal, once, and not when switched off.
	case ${SPHERE_SHELL_INTEGRATION-1} in
		0 | false | off | no) ;;
		*)
			case $- in
				*i*)
					if [ -t 1 ] && [ -n "${__sphere_nonce-}" ] && [ -z "${__sphere_hooked-}" ]; then
						# The DEBUG trap is read and set here and not in a function: bash 3.2 hides it from a
						# function and puts the old one back when the function returns.
						__sphere_old=()
						builtin eval "__sphere_old=( $(builtin trap -p DEBUG) )" 2>/dev/null || :
						__sphere_install "${__sphere_old[2]-}" 2>/dev/null || :
						if [ -n "${__sphere_trap-}" ]; then
							builtin trap -- "$__sphere_trap" DEBUG 2>/dev/null || :
						fi
					fi
					;;
			esac
			;;
	esac
	builtin unset -f __sphere_install 2>/dev/null || :
	builtin unset __sphere_old __sphere_trap 2>/dev/null || :
fi
