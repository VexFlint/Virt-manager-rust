# argcomplete's fish shellcode: run virt-clone with the line in COMP_LINE and
# read the completions back from fd 8, one per line.
function __fish_virt_clone_complete
    set -lx _ARGCOMPLETE 1
    set -lx _ARGCOMPLETE_IFS \n
    set -lx _ARGCOMPLETE_SUPPRESS_SPACE 1
    set -lx COMP_LINE (commandline -p)
    set -lx COMP_POINT (string length -- (commandline -cp))
    set -lx COMP_TYPE
    if set -q _ARC_DEBUG
        virt-clone 8>&1 9>&2 1>&9 2>&1
    else
        virt-clone 8>&1 9>&2 1>/dev/null 2>&1
    end
end
complete -c virt-clone -f -a '(__fish_virt_clone_complete)'
