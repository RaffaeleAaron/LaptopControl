"""Native KDE StatusNotifierItem and DBusMenu; no GTK window or polling loop."""
from pathlib import Path
import os
import socket
import json
from gi.repository import Gio, GLib

ITEM = '''<node><interface name="org.kde.StatusNotifierItem">
<property name="Category" type="s" access="read"/><property name="Id" type="s" access="read"/>
<property name="Title" type="s" access="read"/><property name="Status" type="s" access="read"/>
<property name="IconName" type="s" access="read"/><property name="IconPixmap" type="a(iiay)" access="read"/>
<property name="IconThemePath" type="s" access="read"/><property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
<property name="ItemIsMenu" type="b" access="read"/><property name="Menu" type="o" access="read"/>
<property name="WindowId" type="u" access="read"/>
<method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="Scroll"><arg type="i" direction="in"/><arg type="s" direction="in"/></method>
<method name="ProvideXdgActivationToken"><arg type="s" direction="in"/></method>
<signal name="NewIcon"/><signal name="NewToolTip"/><signal name="NewStatus"><arg type="s"/></signal>
</interface></node>'''
MENU = '''<node><interface name="com.canonical.dbusmenu">
<property name="Version" type="u" access="read"/><property name="TextDirection" type="s" access="read"/>
<property name="Status" type="s" access="read"/><property name="IconThemePath" type="as" access="read"/>
<method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/><arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method>
<method name="GetGroupProperties"><arg type="ai" direction="in"/><arg type="as" direction="in"/><arg type="a(ia{sv})" direction="out"/></method>
<method name="GetProperty"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
<method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method>
<method name="EventGroup"><arg type="a(isvu)" direction="in"/><arg type="ai" direction="out"/></method>
<method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
<method name="AboutToShowGroup"><arg type="ai" direction="in"/><arg type="ai" direction="out"/><arg type="ai" direction="out"/></method>
<signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal>
<signal name="ItemsPropertiesUpdated"><arg type="a(ia{sv})"/><arg type="a(ias)"/></signal>
</interface></node>'''

def variant(value):
    if isinstance(value,bool):return GLib.Variant('b',value)
    if isinstance(value,int):return GLib.Variant('i',value)
    return GLib.Variant('s',value)

def fan_exchange(path, request):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(1.)
        stream.connect(path)
        stream.sendall((json.dumps(request)+'\n').encode())
        with stream.makefile('r') as reader:
            reply=json.loads(reader.readline())
    if reply.get('error'):raise RuntimeError(reply['error'])
    return reply['status']

class Tray:
    def __init__(self, service, fan_socket):
        self.service,self.worker,self.fan_socket=service,service.worker,fan_socket
        self.bus=Gio.bus_get_sync(Gio.BusType.SESSION,None)
        self.revision=1
        self.profile='Unknown'
        self.nodes={}
        self.token=''
        self.icon=self.icon_path()
        self.pixmaps=[]
        if self.icon:
            from PIL import Image
            with Image.open(self.icon) as image:
                image=image.convert('RGBA').resize((32,32))
                red,green,blue,alpha=image.split()
                self.pixmaps=[(32,32,Image.merge('RGBA',(alpha,red,green,blue)).tobytes())]
        self.bus.register_object('/StatusNotifierItem',Gio.DBusNodeInfo.new_for_xml(ITEM).interfaces[0],self.item_call,self.item_property,None)
        self.bus.register_object('/MenuBar',Gio.DBusNodeInfo.new_for_xml(MENU).interfaces[0],self.menu_call,self.menu_property,None)
        Gio.bus_watch_name_on_connection(self.bus,'org.kde.StatusNotifierWatcher',Gio.BusNameWatcherFlags.NONE,self.register,None)
        self.build_menu()

    def icon_path(self):
        here=Path(__file__).resolve().parent
        for path in [here.parent/'icons/hicolor/256x256/apps/laptop-control.png',here.parent/'assets/laptop-control.png',Path('/usr/local/share/icons/hicolor/256x256/apps/laptop-control.png')]:
            if path.exists():return str(path)
        return ''

    def register(self, connection, name, owner):
        connection.call('org.kde.StatusNotifierWatcher','/StatusNotifierWatcher','org.kde.StatusNotifierWatcher','RegisterStatusNotifierItem',GLib.Variant('(s)',(self.bus.get_unique_name(),)),None,Gio.DBusCallFlags.NONE,-1,None,None,None)

    def item_property(self, connection, sender, path, interface, name):
        source='on' if self.worker.state=='Live' else 'off'
        output='on' if self.worker.virtual.enabled else 'off'
        values={
            'Category':GLib.Variant('s','Hardware'),'Id':GLib.Variant('s','laptop-control'),
            'Title':GLib.Variant('s','Laptop Control'),'Status':GLib.Variant('s','Active'),
            'IconName':GLib.Variant('s',self.icon or 'computer'),'IconPixmap':GLib.Variant('a(iiay)',self.pixmaps),
            'IconThemePath':GLib.Variant('s',str(Path(self.icon).parent)),
            'ToolTip':GLib.Variant('(sa(iiay)ss)',(self.icon,[], 'Laptop Control',f'Fans: {self.profile} | Camera: {source} | Virtual output: {output}')),
            'ItemIsMenu':GLib.Variant('b',False),'Menu':GLib.Variant('o','/MenuBar'),'WindowId':GLib.Variant('u',0),
        }
        return values.get(name)

    def item_call(self,connection,sender,path,interface,method,params,invocation):
        if method in ('Activate','SecondaryActivate'):
            token,self.token=self.token,''
            self.service.show_gui(token=token)
        elif method=='ProvideXdgActivationToken':self.token=params.unpack()[0]
        invocation.return_value(None)

    def menu_property(self,connection,sender,path,interface,name):
        return {'Version':GLib.Variant('u',4),'TextDirection':GLib.Variant('s','ltr'),'Status':GLib.Variant('s','normal'),'IconThemePath':GLib.Variant('as',[str(Path(self.icon).parent)])}.get(name)

    def build_menu(self, refresh_fans=False):
        if refresh_fans:
            try:self.profile=fan_exchange(self.fan_socket,'Status')['profiles']['selected']
            except (OSError,ValueError,RuntimeError):self.profile='Unknown'
        book=self.service.load_book()
        self.nodes={}
        def node(number,label='',children=(),**props):
            properties={'label':label,'enabled':True,'visible':True,**props}
            if children:properties['children-display']='submenu'
            result=(number,{key:variant(v) for key,v in properties.items()},[GLib.Variant('(ia{sv}av)',c) for c in children])
            self.nodes[number]=result
            return result
        fans=[node(11+i,name,**{'toggle-type':'radio','toggle-state':int(name==self.profile),'enabled':self.profile!='Unknown'}) for i,name in enumerate(['Firmware','Office','Gaming'])]
        scenes=[node(1000+i,scene.get('name',f'Scene {i+1}'),**{'toggle-type':'radio','toggle-state':int(i==book.get('selected_scene',0))}) for i,scene in enumerate(book['scenes'])]
        webcam=[node(21,'Stop camera' if self.worker.state=='Live' else 'Start camera'),node(22,'Virtual camera output',**{'toggle-type':'checkmark','toggle-state':int(self.worker.virtual.enabled)}),node(23,**{'type':'separator'}),*scenes]
        return node(0,children=[node(1,'Open Laptop Control'),node(2,**{'type':'separator'}),node(10,'Fan Control',fans),node(20,'Webcam',webcam),node(30,**{'type':'separator'}),node(31,'Quit Laptop Control')])

    def changed(self):
        self.revision+=1
        self.build_menu()
        self.bus.emit_signal(None,'/MenuBar','com.canonical.dbusmenu','LayoutUpdated',GLib.Variant('(ui)',(self.revision,0)))
        self.bus.emit_signal(None,'/StatusNotifierItem','org.kde.StatusNotifierItem','NewToolTip',None)

    def action(self, number):
        try:
            if number==1:
                token,self.token=self.token,''
                self.service.show_gui(token=token)
            elif 11<=number<=13:
                self.profile=fan_exchange(self.fan_socket,{'SelectProfile':['Firmware','Office','Gaming'][number-11]})['profiles']['selected']
            elif number==21:
                if self.worker.state=='Live':
                    self.worker.handle({'op':'stop'});self.service.live_book=None
                else:
                    book=self.service.load_book()
                    self.worker.handle({'op':'start','config':self.service.payload(book)})
                    if not self.worker.error:self.service.live_book=book
            elif number==22:self.worker.handle({'op':'virtual','enabled':not self.worker.virtual.enabled,'config':self.service.payload(self.service.load_book())})
            elif number>=1000:self.service.select_scene(number-1000)
            elif number==31:self.service.request_quit()
            self.changed()
        except Exception as error:
            self.worker.error=str(error);self.worker.snapshot()

    def menu_call(self,connection,sender,path,interface,method,params,invocation):
        try:
            args=params.unpack()
            if method=='GetLayout':
                self.build_menu(refresh_fans=True)
                root=self.nodes.get(args[0],self.nodes[0])
                def depth(node,n):
                    if n==0:return(node[0],node[1],[])
                    if n<0:return node
                    return(node[0],node[1],[GLib.Variant('(ia{sv}av)',depth(self.nodes[c.unpack()[0]],n-1)) for c in node[2]])
                invocation.return_value(GLib.Variant('(u(ia{sv}av))',(self.revision,depth(root,args[1]))))
            elif method=='GetGroupProperties':invocation.return_value(GLib.Variant('(a(ia{sv}))',([(i,self.nodes[i][1]) for i in args[0] if i in self.nodes],)))
            elif method=='GetProperty':invocation.return_value(GLib.Variant('(v)',(self.nodes[args[0]][1][args[1]],)))
            elif method=='Event':
                if args[1]=='clicked':self.action(args[0])
                invocation.return_value(None)
            elif method=='EventGroup':
                for event in args[0]:
                    if event[1]=='clicked':self.action(event[0])
                invocation.return_value(GLib.Variant('(ai)',([],)))
            elif method=='AboutToShow':
                self.build_menu(refresh_fans=True);invocation.return_value(GLib.Variant('(b)',(True,)))
            elif method=='AboutToShowGroup':
                self.build_menu(refresh_fans=True);invocation.return_value(GLib.Variant('(aiai)',(args[0],[])))
            else:invocation.return_dbus_error('org.freedesktop.DBus.Error.UnknownMethod',method)
        except Exception as error:invocation.return_dbus_error('org.freedesktop.DBus.Error.Failed',str(error))
