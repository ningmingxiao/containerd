# -*- coding: utf-8 -*-
import commands
import io
import  datetime


def run_with_output(cmd):
    return commands.getoutput(cmd)

def delete_merge_time(file):
    newlines = []
    with io.open(file, 'r',encoding="utf-8") as f:
        line_count =0
        line_need_del =[]
        lines =  f.readlines()
        for line in lines:
            line_count +=1
            line_list = line.split(" ")
            # delete time
            if line_list[0] == "*":
                del line_list[4]
            # delete merge message
            if (line_list[0] == "-" ):
                if line_list[1].startswith("Merge") :
                    line_del_first = line_count -1
                    line_del_second = line_count
                    line_del_third = line_count+1
                    line_need_del.append(line_del_first)
                    line_need_del.append(line_del_second)
                    line_need_del.append(line_del_third)
            new_line = " ".join(line_list)
            newlines.append(new_line)
    for dele_line in line_need_del:
        index=dele_line-1
        newlines[index] = ""
    result= []
    for data in newlines:
        if data != "":
            result.append(data)
    with io.open(file, 'w',encoding="utf-8") as f:
        f.writelines(result)


def generate_changelog(file):
    newlines = []
    with io.open(file, 'r',encoding="utf-8") as f:
        line_count =0
        for line in f.readlines():
            line_count +=1
            line_list = line.split(" ")
            if (line_list[0] == "-" ):
                tfs_ec= ""
                cve = ""
                if line_list [1].startswith("TFS") or line_list [1].startswith("EC")  :
                    tfs_ec ="["+line_list[1]+"]"
                for d in line_list:
                    if d.startswith("CVE-"):
                        cve = "{"+d+"}"
                line_list[-1] =line_list[-1].strip('\n') +" " + tfs_ec+" " +cve +"\n"
            new_line = " ".join(line_list)
            newlines.append(new_line)
    with io.open(file, 'w',encoding="utf-8") as f:
        f.writelines(newlines)
    run_with_output("cat "+file + ">> /root/rpmbuild/SPECS/containerd.spec")


def generate_runc_gitlog(logpath):
    run_with_output('cd /root/rpmbuild/runc &&git log --after="2022-01-05 00:00:00" --format="* %cd %aN<%ae> %n- %s%d%n" --date=local  >'+logpath)
    # add RUNC tag
    with io.open(logpath, 'r',encoding="utf-8") as f:
        lines = f.readlines()
        for i in range(0,len(lines)):
            if lines[i].startswith("*") and lines[i+1].startswith("-"):
                with_runc = lines[i+1][:-1]+" RUNC\n"
                lines[i+1] = with_runc
    with io.open(logpath, 'w',encoding="utf-8") as f:
        f.writelines(lines)

def sort_gitlog(filename):
    with io.open(filename, 'r',encoding="utf-8") as f:
        lines = f.readlines()
        list_all =[]
        for i in range(0,len(lines)):
            dict_temp = {}
            if lines[i].startswith("*") and lines[i+1].startswith("-"):
                dict_temp["time_author"]= lines[i]
                dict_temp["content"]= lines[i+1]
                dict_temp["newline"] = "\n"
                time_now = lines[i].split(" ")[1:6]
                dict_temp["time"]= str(datetime.datetime.strptime(" ".join(time_now),'%a %b %d %H:%M:%S %Y'))
                list_all.append(dict_temp)
    sorted_date = sorted(list_all, key=lambda x: datetime.datetime.strptime(x['time'], '%Y-%m-%d %H:%M:%S'), reverse=True)
    new_liness = []
    for data in sorted_date:
        line = data["time_author"]+data["content"] + data["newline"]
        new_liness.append(line)
    with io.open(filename, 'w',encoding="utf-8") as f:
        f.writelines(new_liness)

if __name__ == '__main__':
    gitlog = "/root/rpmbuild/SPECS/gitlog"
    generate_runc_gitlog(gitlog)
    run_with_output('cd /root/rpmbuild/containerd.io &&git log --after="2022-01-05 00:00:00" --format="* %cd %aN<%ae> %n- %s%d%n" --date=local  >>' +gitlog)
    sort_gitlog(gitlog)
    delete_merge_time(gitlog)
    generate_changelog(gitlog)
